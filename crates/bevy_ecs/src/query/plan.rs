use crate::{
    component::ComponentId,
    entity::Entity,
    query::FilteredAccess,
    world::unsafe_world_cell::UnsafeWorldCell,
};
use alloc::vec::Vec;
use bevy_ptr::Ptr;

/// This enum describes a way to access the entities of Relationship and RelationshipTarget components
/// in a type-erased context.
#[derive(Debug, Clone, Copy)]
pub enum RelationshipAccessor {
    /// This component is a Relationship.
    Relationship {
        /// Offset of the field containing Entity from the base of the component.
        entity_field_offset: usize,
        /// Value of RelationshipTarget::LINKED_SPAWN for the RelationshipTarget of this Relationship.
        linked_spawn: bool,
    },
    /// This component is a RelationshipTarget.
    RelationshipTarget {
        /// Function that returns an iterator over all Entitys of this RelationshipTarget's collection.
        ///
        /// # Safety
        /// Passed pointer must point to the value of the same component as the one that this accessor was registered to.
        iter: for<'a> unsafe fn(Ptr<'a>) -> alloc::boxed::Box<dyn Iterator<Item = Entity> + 'a>,
        /// Value of RelationshipTarget::LINKED_SPAWN of this RelationshipTarget.
        linked_spawn: bool,
    },
}

/// Represents a single term/source in a multi-source query.
/// Each term has its own ComponentAccess requirements.
#[derive(Debug, Clone)]
pub struct QueryTerm {
    /// The components that need to be accessed for this term.
    pub access: FilteredAccess,
    /// Index of this term in the query plan.
    pub term_index: usize,
}

impl QueryTerm {
    /// Create a new query term with the given access.
    pub fn new(term_index: usize, access: FilteredAccess) -> Self {
        Self { access, term_index }
    }

    /// Check if an entity matches this term's requirements.
    ///
    /// # Safety
    /// - `entity` must be valid in the world
    /// - Caller must ensure proper access to the entity's components
    pub unsafe fn matches(&self, entity: Entity, world: UnsafeWorldCell) -> bool {
        let Some(location) = world.entities().get(entity) else {
            return false;
        };

        let archetype = world.archetypes().get(location.archetype_id).unwrap();

        // Check if the archetype contains all required components
        if let Ok(components) = self.access.access().try_iter_component_access() {
            for component_access in components {
                let component_id = match component_access {
                    crate::query::ComponentAccessKind::Exclusive(id) => id,
                    crate::query::ComponentAccessKind::Shared(id) => id,
                    crate::query::ComponentAccessKind::Archetypal(id) => id,
                };
                if !archetype.contains(component_id) {
                    return false;
                }
            }
        } else {
            // If we have inverted access (e.g., read_all), we can't check specific components
            // This should be handled differently in a real implementation
            return false;
        }

        true
    }
}

/// Describes how two query terms are connected via a relationship.
#[derive(Debug, Clone)]
pub struct QueryRelationship {
    /// The source term index.
    pub source_term: usize,
    /// The target term index.
    pub target_term: usize,
    /// The relationship component that links source to target.
    pub relationship_component: ComponentId,
    /// How to access the relationship data.
    pub accessor: RelationshipAccessor,
}

impl QueryRelationship {
    /// Get the related entities from a source entity.
    ///
    /// # Safety
    /// - `source_entity` must be valid and have the relationship component
    /// - Caller must ensure proper access to the relationship component
    pub unsafe fn get_related_entities(
        &self,
        source_entity: Entity,
        world: UnsafeWorldCell,
    ) -> Vec<Entity> {
        let Some(location) = world.entities().get(source_entity) else {
            return Vec::new();
        };

        let archetype = world.archetypes().get(location.archetype_id).unwrap();

        if !archetype.contains(self.relationship_component) {
            return Vec::new();
        }

        let component_ptr = match archetype.get_storage_type(self.relationship_component) {
            Some(crate::component::StorageType::Table) => {
                let table = world.storages().tables.get(archetype.table_id()).unwrap();
                table
                    .get_component(self.relationship_component, location.table_row)
                    .unwrap()
            }
            Some(crate::component::StorageType::SparseSet) => {
                let sparse_set = world
                    .storages()
                    .sparse_sets
                    .get(self.relationship_component)
                    .unwrap();
                sparse_set.get(source_entity).unwrap()
            }
            None => return Vec::new(),
        };

        match &self.accessor {
            RelationshipAccessor::Relationship { entity_field_offset, .. } => {
                // For Relationship components, read the entity at the offset
                let entity_ptr = component_ptr.byte_add(*entity_field_offset);
                let target_entity: Entity = *entity_ptr.deref();
                alloc::vec![target_entity]
            }
            RelationshipAccessor::RelationshipTarget { iter, .. } => {
                // For RelationshipTarget components, use the iterator
                iter(component_ptr).collect()
            }
        }
    }
}

/// A dynamic query plan that describes how to match multiple entities
/// connected through relationships.
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// All terms in this query.
    pub terms: Vec<QueryTerm>,
    /// Relationships that connect the terms.
    pub relationships: Vec<QueryRelationship>,
    /// The index of the main term (the one we iterate over).
    pub main_term_index: usize,
}

impl QueryPlan {
    /// Create a new empty query plan.
    pub fn new(main_term_index: usize) -> Self {
        Self {
            terms: Vec::new(),
            relationships: Vec::new(),
            main_term_index,
        }
    }

    /// Add a term to the query plan.
    pub fn add_term(&mut self, access: FilteredAccess) -> usize {
        let term_index = self.terms.len();
        self.terms.push(QueryTerm::new(term_index, access));
        term_index
    }

    /// Add a relationship between two terms.
    pub fn add_relationship(
        &mut self,
        source_term: usize,
        target_term: usize,
        relationship_component: ComponentId,
        accessor: RelationshipAccessor,
    ) {
        self.relationships.push(QueryRelationship {
            source_term,
            target_term,
            relationship_component,
            accessor,
        });
    }

    /// Get the access for the main term (used for archetype matching).
    pub fn main_term_access(&self) -> &FilteredAccess {
        &self.terms[self.main_term_index].access
    }

    /// Execute the query plan for a given main entity, returning all matched entity sets.
    ///
    /// Returns a Vec of entity arrays, where each array contains one entity per term.
    ///
    /// # Safety
    /// - `main_entity` must be valid in the world
    /// - Caller must ensure proper access to all components in the plan
    pub unsafe fn execute(
        &self,
        main_entity: Entity,
        world: UnsafeWorldCell,
    ) -> Vec<Vec<Entity>> {
        let mut results = Vec::new();

        // Start with the main entity
        let mut partial_match = alloc::vec![None; self.terms.len()];
        partial_match[self.main_term_index] = Some(main_entity);

        // Check if main entity matches its term
        if !self.terms[self.main_term_index].matches(main_entity, world) {
            return results;
        }

        // Recursively resolve all relationships
        self.resolve_term(self.main_term_index, &mut partial_match, world, &mut results);

        results
    }

    /// Recursively resolve a term and all its connected terms.
    ///
    /// # Safety
    /// - All entities in `partial_match` must be valid
    /// - Caller must ensure proper access to all components
    unsafe fn resolve_term(
        &self,
        term_index: usize,
        partial_match: &mut Vec<Option<Entity>>,
        world: UnsafeWorldCell,
        results: &mut Vec<Vec<Entity>>,
    ) {
        // Find all relationships from this term
        let outgoing_relationships: Vec<_> = self
            .relationships
            .iter()
            .filter(|r| r.source_term == term_index)
            .collect();

        if outgoing_relationships.is_empty() {
            // No more relationships to resolve, check if we have a complete match
            if partial_match.iter().all(|e| e.is_some()) {
                results.push(partial_match.iter().map(|e| e.unwrap()).collect());
            }
            return;
        }

        // Process each outgoing relationship
        for relationship in outgoing_relationships {
            let source_entity = partial_match[term_index].unwrap();
            let related_entities = relationship.get_related_entities(source_entity, world);

            for target_entity in related_entities {
                // Check if target entity matches its term
                if !self.terms[relationship.target_term].matches(target_entity, world) {
                    continue;
                }

                // Save the current state
                let previous = partial_match[relationship.target_term];
                partial_match[relationship.target_term] = Some(target_entity);

                // Recursively resolve from the target term
                self.resolve_term(relationship.target_term, partial_match, world, results);

                // Restore state for backtracking
                partial_match[relationship.target_term] = previous;
            }
        }
    }

    /// Get the combined access for all terms in the plan.
    pub fn combined_access(&self) -> FilteredAccess {
        let mut combined = FilteredAccess::matches_everything();
        for term in &self.terms {
            combined.extend(&term.access);
        }
        combined
    }
}

/// A builder for constructing a [`QueryPlan`].
///
/// # Example
/// ```
/// # use bevy_ecs::prelude::*;
/// # use bevy_ecs::query::{QueryPlanBuilder, RelationshipAccessor, FilteredAccess};
/// # use bevy_ecs::component::ComponentId;
/// # let mut world = World::new();
/// # let spaceship_id = world.register_component::<()>();
/// # let faction_id = world.register_component::<()>();
/// # let planet_id = world.register_component::<()>();
/// let mut builder = QueryPlanBuilder::new();
///
/// // Term 0: Spaceship (main source)
/// let mut access0 = FilteredAccess::matches_everything();
/// access0.add_component_read(spaceship_id);
/// let term0 = builder.add_term(access0);
///
/// // Term 1: Faction
/// let mut access1 = FilteredAccess::matches_everything();
/// let term1 = builder.add_term(access1);
///
/// // Relationship: Faction from Spaceship to Faction entity
/// builder.add_relationship(
///     term0,
///     term1,
///     faction_id,
///     RelationshipAccessor::Relationship {
///         entity_field_offset: 0,
///         linked_spawn: false,
///     },
/// );
///
/// let plan = builder.build(term0);
/// ```
pub struct QueryPlanBuilder {
    terms: Vec<QueryTerm>,
    relationships: Vec<QueryRelationship>,
}

impl QueryPlanBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            terms: Vec::new(),
            relationships: Vec::new(),
        }
    }

    /// Add a term to the query plan.
    /// Returns the term index which can be used in relationships.
    pub fn add_term(&mut self, access: FilteredAccess) -> usize {
        let term_index = self.terms.len();
        self.terms.push(QueryTerm::new(term_index, access));
        term_index
    }

    /// Add a relationship between two terms.
    pub fn add_relationship(
        &mut self,
        source_term: usize,
        target_term: usize,
        relationship_component: ComponentId,
        accessor: RelationshipAccessor,
    ) {
        self.relationships.push(QueryRelationship {
            source_term,
            target_term,
            relationship_component,
            accessor,
        });
    }

    /// Build the final query plan with the specified main term.
    pub fn build(self, main_term_index: usize) -> QueryPlan {
        QueryPlan {
            terms: self.terms,
            relationships: self.relationships,
            main_term_index,
        }
    }
}

impl Default for QueryPlanBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A typed builder for constructing query plans with compile-time component type information.
///
/// This provides a more ergonomic API compared to the low-level `QueryPlanBuilder`.
///
/// # Example
/// ```
/// # use bevy_ecs::prelude::*;
/// # use bevy_ecs::query::TypedQueryPlanBuilder;
/// # use bevy_ecs::hierarchy::ChildOf;
/// #
/// # #[derive(Component)]
/// # struct SpaceShip;
/// # #[derive(Component)]
/// # struct Faction(Entity);
/// #
/// # let mut world = World::new();
/// let mut builder = TypedQueryPlanBuilder::new(&mut world);
///
/// // Add terms with typed component access
/// let spaceship_term = builder.with::<SpaceShip>();
/// let faction_term = builder.term();
///
/// // Add a typed relationship
/// builder.related_to::<Faction>(spaceship_term, faction_term);
///
/// let plan = builder.build(spaceship_term);
/// ```
pub struct TypedQueryPlanBuilder<'w> {
    world: &'w mut World,
    builder: QueryPlanBuilder,
}

impl<'w> TypedQueryPlanBuilder<'w> {
    /// Create a new typed builder.
    pub fn new(world: &'w mut World) -> Self {
        Self {
            world,
            builder: QueryPlanBuilder::new(),
        }
    }

    /// Add a term that queries entities with a specific component.
    pub fn with<T: crate::component::Component>(&mut self) -> usize {
        let component_id = self.world.register_component::<T>();
        let mut access = FilteredAccess::matches_everything();
        access.add_component_read(component_id);
        self.builder.add_term(access)
    }

    /// Add a term that requires mutable access to a specific component.
    pub fn with_mut<T: crate::component::Component>(&mut self) -> usize {
        let component_id = self.world.register_component::<T>();
        let mut access = FilteredAccess::matches_everything();
        access.add_component_write(component_id);
        self.builder.add_term(access)
    }

    /// Add an empty term (no component requirements).
    pub fn term(&mut self) -> usize {
        let access = FilteredAccess::matches_everything();
        self.builder.add_term(access)
    }

    /// Add additional read access to a component for an existing term.
    pub fn add_read<T: crate::component::Component>(&mut self, term_index: usize) {
        let component_id = self.world.register_component::<T>();
        self.builder.terms[term_index].access.add_component_read(component_id);
    }

    /// Add additional write access to a component for an existing term.
    pub fn add_write<T: crate::component::Component>(&mut self, term_index: usize) {
        let component_id = self.world.register_component::<T>();
        self.builder.terms[term_index].access.add_component_write(component_id);
    }

    /// Add a Without filter to a term.
    pub fn without<T: crate::component::Component>(&mut self, term_index: usize) {
        let component_id = self.world.register_component::<T>();
        self.builder.terms[term_index].access.and_without(component_id);
    }

    /// Add a relationship between two terms using a typed Relationship component.
    pub fn related_to<R: crate::relationship::Relationship>(
        &mut self,
        source_term: usize,
        target_term: usize,
    ) {
        let component_id = self.world.register_component::<R>();

        // Get the relationship accessor from component info
        use core::mem::offset_of;
        // For simple relationships that are a newtype around Entity, the offset is 0
        // TODO: In a real implementation, this should use component metadata
        let accessor = RelationshipAccessor::Relationship {
            entity_field_offset: 0,
            linked_spawn: R::RelationshipTarget::LINKED_SPAWN,
        };

        self.builder.add_relationship(
            source_term,
            target_term,
            component_id,
            accessor,
        );
    }

    /// Build the final query plan.
    pub fn build(self, main_term_index: usize) -> QueryPlan {
        self.builder.build(main_term_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        component::Component,
        hierarchy::ChildOf,
        prelude::World,
    };

    #[derive(Component)]
    struct Marker;

    #[test]
    fn test_query_plan_basic() {
        let mut world = World::new();

        // Create parent-child relationship
        let parent = world.spawn_empty().id();
        let child = world.spawn((Marker, ChildOf(parent))).id();
        world.flush(); // Ensure Children component is added to parent

        let marker_id = world.register_component::<Marker>();
        let child_of_id = world.register_component::<ChildOf>();

        // Build a simple plan using the builder API
        let mut builder = QueryPlanBuilder::new();

        // Term 0: Entities with Marker (main term)
        let mut access0 = FilteredAccess::matches_everything();
        access0.add_component_read(marker_id);
        let term0 = builder.add_term(access0);

        // Term 1: Parent entities
        let access1 = FilteredAccess::matches_everything();
        let term1 = builder.add_term(access1);

        // Relationship: ChildOf from term 0 to term 1
        use core::mem::offset_of;
        let accessor = RelationshipAccessor::Relationship {
            entity_field_offset: offset_of!(ChildOf, 0),
            linked_spawn: true,
        };
        builder.add_relationship(term0, term1, child_of_id, accessor);

        let plan = builder.build(term0);

        // Execute the plan
        unsafe {
            let world_cell = world.as_unsafe_world_cell_readonly();
            let results = plan.execute(child, world_cell);

            assert_eq!(results.len(), 1);
            assert_eq!(results[0][0], child);
            assert_eq!(results[0][1], parent);
        }
    }

    #[test]
    fn test_query_plan_builder() {
        let mut world = World::new();

        let marker_id = world.register_component::<Marker>();

        // Build a simple single-term plan
        let mut builder = QueryPlanBuilder::new();
        let mut access = FilteredAccess::matches_everything();
        access.add_component_read(marker_id);
        let term = builder.add_term(access);
        let plan = builder.build(term);

        assert_eq!(plan.terms.len(), 1);
        assert_eq!(plan.relationships.len(), 0);
        assert_eq!(plan.main_term_index, 0);
    }

    #[test]
    fn test_typed_query_plan_builder() {
        let mut world = World::new();

        // Build a plan using the typed API
        let mut builder = TypedQueryPlanBuilder::new(&mut world);
        let child_term = builder.with::<Marker>();
        let parent_term = builder.term();
        builder.related_to::<ChildOf>(child_term, parent_term);

        let plan = builder.build(child_term);

        assert_eq!(plan.terms.len(), 2);
        assert_eq!(plan.relationships.len(), 1);
        assert_eq!(plan.main_term_index, 0);

        // Test the plan works
        let parent = world.spawn_empty().id();
        let child = world.spawn((Marker, ChildOf(parent))).id();
        world.flush();

        unsafe {
            let world_cell = world.as_unsafe_world_cell_readonly();
            let results = plan.execute(child, world_cell);

            assert_eq!(results.len(), 1);
            assert_eq!(results[0][0], child);
            assert_eq!(results[0][1], parent);
        }
    }

    #[derive(Component)]
    struct Name;

    #[derive(Component)]
    struct Position;

    #[test]
    fn test_typed_builder_multiple_components() {
        let mut world = World::new();

        // Build a plan with multiple components on a single term
        let mut builder = TypedQueryPlanBuilder::new(&mut world);

        // Term 0: Entities with Marker, Name, and Position
        let term0 = builder.with::<Marker>();
        builder.add_read::<Name>(term0);
        builder.add_read::<Position>(term0);

        let plan = builder.build(term0);

        assert_eq!(plan.terms.len(), 1);

        // Verify the term has access to all three components
        let access = &plan.terms[0].access;
        assert!(access.access().has_component_read(world.register_component::<Marker>()));
        assert!(access.access().has_component_read(world.register_component::<Name>()));
        assert!(access.access().has_component_read(world.register_component::<Position>()));
    }
}

