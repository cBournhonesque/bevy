use crate::{
    component::ComponentId,
    entity::Entity,
    query::FilteredAccess,
    world::unsafe_world_cell::UnsafeWorldCell,
};
use alloc::vec::Vec;
use std::marker::PhantomData;
use fixedbitset::FixedBitSet;
use bevy_ecs::component::{Component, StorageType};
use bevy_ecs::prelude::{Query, QueryState, With, Without, World};
use bevy_ecs::query::{QueryData, QueryFilter};
use bevy_ptr::Ptr;


pub struct QueryBuilder {
    access: FilteredAccess,
    or: bool,
    first: bool,
}

impl QueryBuilder {
    /// Creates a new builder with the accesses required for `Q` and `F`
    pub fn new(world: &'w mut World) -> Self {
        let fetch_state = D::init_state(world);
        let filter_state = F::init_state(world);

        let mut access = FilteredAccess::default();
        D::update_component_access(&fetch_state, &mut access);

        // Use a temporary empty FilteredAccess for filters. This prevents them from conflicting with the
        // main Query's `fetch_state` access. Filters are allowed to conflict with the main query fetch
        // because they are evaluated *before* a specific reference is constructed.
        let mut filter_access = FilteredAccess::default();
        F::update_component_access(&filter_state, &mut filter_access);

        // Merge the temporary filter access with the main access. This ensures that filter access is
        // properly considered in a global "cross-query" context (both within systems and across systems).
        access.extend(&filter_access);

        Self {
            access,
            world,
            or: false,
            first: false,
            _marker: PhantomData,
        }
    }

    pub(super) fn is_dense(&self) -> bool {
        // Note: `component_id` comes from the user in safe code, so we cannot trust it to
        // exist. If it doesn't exist we pessimistically assume it's sparse.
        let is_dense = |component_id| {
            self.world()
                .components()
                .get_info(component_id)
                .is_some_and(|info| info.storage_type() == StorageType::Table)
        };

        let Ok(component_accesses) = self.access.access().try_iter_component_access() else {
            // Access is unbounded, pessimistically assume it's sparse.
            return false;
        };

        component_accesses
            .map(|access| *access.index())
            .all(is_dense)
            && !self.access.access().has_read_all_components()
            && self.access.with_filters().all(is_dense)
            && self.access.without_filters().all(is_dense)
    }

    /// Returns a reference to the world passed to [`Self::new`].
    pub fn world(&self) -> &World {
        self.world
    }

    /// Returns a mutable reference to the world passed to [`Self::new`].
    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }

    /// Adds access to self's underlying [`FilteredAccess`] respecting [`Self::or`] and [`Self::and`]
    pub fn extend_access(&mut self, mut access: FilteredAccess) {
        if self.or {
            if self.first {
                access.required.clear();
                self.access.extend(&access);
                self.first = false;
            } else {
                self.access.append_or(&access);
            }
        } else {
            self.access.extend(&access);
        }
    }

    /// Adds accesses required for `T` to self.
    pub fn data<T: QueryData>(&mut self) -> &mut Self {
        let state = T::init_state(self.world);
        let mut access = FilteredAccess::default();
        T::update_component_access(&state, &mut access);
        self.extend_access(access);
        self
    }

    /// Adds filter from `T` to self.
    pub fn filter<T: QueryFilter>(&mut self) -> &mut Self {
        let state = T::init_state(self.world);
        let mut access = FilteredAccess::default();
        T::update_component_access(&state, &mut access);
        self.extend_access(access);
        self
    }

    /// Adds [`With<T>`] to the [`FilteredAccess`] of self.
    pub fn with<T: Component>(&mut self) -> &mut Self {
        self.filter::<With<T>>();
        self
    }

    /// Adds [`With<T>`] to the [`FilteredAccess`] of self from a runtime [`ComponentId`].
    pub fn with_id(&mut self, id: ComponentId) -> &mut Self {
        let mut access = FilteredAccess::default();
        access.and_with(id);
        self.extend_access(access);
        self
    }

    /// Adds [`Without<T>`] to the [`FilteredAccess`] of self.
    pub fn without<T: Component>(&mut self) -> &mut Self {
        self.filter::<Without<T>>();
        self
    }

    /// Adds [`Without<T>`] to the [`FilteredAccess`] of self from a runtime [`ComponentId`].
    pub fn without_id(&mut self, id: ComponentId) -> &mut Self {
        let mut access = FilteredAccess::default();
        access.and_without(id);
        self.extend_access(access);
        self
    }

    /// Adds `&T` to the [`FilteredAccess`] of self.
    pub fn ref_id(&mut self, id: ComponentId) -> &mut Self {
        self.with_id(id);
        self.access.add_component_read(id);
        self
    }

    /// Adds `&mut T` to the [`FilteredAccess`] of self.
    pub fn mut_id(&mut self, id: ComponentId) -> &mut Self {
        self.with_id(id);
        self.access.add_component_write(id);
        self
    }

    /// Takes a function over mutable access to a [`bevy_ecs::prelude::QueryBuilder`], calls that function
    /// on an empty builder and then adds all accesses from that builder to self as optional.
    pub fn optional(&mut self, f: impl Fn(&mut bevy_ecs::prelude::QueryBuilder)) -> &mut Self {
        let mut builder = bevy_ecs::prelude::QueryBuilder::new(self.world);
        f(&mut builder);
        self.access.extend_access(builder.access());
        self
    }

    /// Takes a function over mutable access to a [`bevy_ecs::prelude::QueryBuilder`], calls that function
    /// on an empty builder and then adds all accesses from that builder to self.
    ///
    /// Primarily used when inside a [`Self::or`] closure to group several terms.
    pub fn and(&mut self, f: impl Fn(&mut bevy_ecs::prelude::QueryBuilder)) -> &mut Self {
        let mut builder = bevy_ecs::prelude::QueryBuilder::new(self.world);
        f(&mut builder);
        let access = builder.access().clone();
        self.extend_access(access);
        self
    }

    /// Takes a function over mutable access to a [`bevy_ecs::prelude::QueryBuilder`], calls that function
    /// on an empty builder, all accesses added to that builder will become terms in an or expression.
    ///
    /// ```
    /// # use bevy_ecs::prelude::*;
    /// #
    /// # #[derive(Component)]
    /// # struct A;
    /// #
    /// # #[derive(Component)]
    /// # struct B;
    /// #
    /// # let mut world = World::new();
    /// #
    /// QueryBuilder::<Entity>::new(&mut world).or(|builder| {
    ///     builder.with::<A>();
    ///     builder.with::<B>();
    /// });
    /// // is equivalent to
    /// QueryBuilder::<Entity>::new(&mut world).filter::<Or<(With<A>, With<B>)>>();
    /// ```
    pub fn or(&mut self, f: impl Fn(&mut bevy_ecs::prelude::QueryBuilder)) -> &mut Self {
        let mut builder = bevy_ecs::prelude::QueryBuilder::new(self.world);
        builder.or = true;
        builder.first = true;
        f(&mut builder);
        self.access.extend(builder.access());
        self
    }

    /// Returns a reference to the [`FilteredAccess`] that will be provided to the built [`Query`].
    pub fn access(&self) -> &FilteredAccess {
        &self.access
    }

    /// Transmute the existing builder adding required accesses.
    /// This will maintain all existing accesses.
    ///
    /// If including a filter type see [`Self::transmute_filtered`]
    pub fn transmute<NewD: QueryData>(&mut self) -> &mut bevy_ecs::prelude::QueryBuilder<'w, NewD> {
        self.transmute_filtered::<NewD, ()>()
    }

    /// Transmute the existing builder adding required accesses.
    /// This will maintain all existing accesses.
    pub fn transmute_filtered<NewD: QueryData, NewF: QueryFilter>(
        &mut self,
    ) -> &mut bevy_ecs::prelude::QueryBuilder<'w, NewD, NewF> {
        let fetch_state = NewD::init_state(self.world);
        let filter_state = NewF::init_state(self.world);

        let mut access = FilteredAccess::default();
        NewD::update_component_access(&fetch_state, &mut access);
        NewF::update_component_access(&filter_state, &mut access);

        self.extend_access(access);
        // SAFETY:
        // - We have included all required accesses for NewQ and NewF
        // - The layout of all QueryBuilder instances is the same
        unsafe { core::mem::transmute(self) }
    }

    /// Create a [`QueryState`] with the accesses of the builder.
    ///
    /// Takes `&mut self` to access the inner world reference while initializing
    /// state for the new [`QueryState`]
    pub fn build(&mut self) -> QueryState<D, F> {
        QueryState::<D, F>::from_builder(self)
    }
}

/// Represents a single source in a multi-source query.
/// Each term has its own ComponentAccess requirements.
#[derive(Debug, Clone)]
pub struct QueryElement {
    /// The components that need to be accessed for this term.
    pub access: FilteredAccess,
    /// Index of this term in the query plan.
    pub term_index: usize,
}

impl QueryElement {
    /// Create a new query term with the given access.
    pub fn new(term_index: usize, access: FilteredAccess) -> Self {
        Self { access, term_index, relationships }
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

pub struct QueryVariable {
    index: u8,
}

impl From<u8> for QueryVariable {
    fn from(value: u8) -> Self {
        Self {
            index: value
        }
    }
}

pub struct QueryPlanBuilder<'w, 'p> {
    world: &'w mut World,
    plan: QueryPlan,
}

impl<'w, 'p> QueryPlanBuilder {
    pub fn new(world: &'w mut World) -> Self {
        Self {
            world,
            plan: QueryPlan::default(),
        }
    }

    pub fn add_source<D: QueryData, F: QueryFilter>(&mut self) -> &mut Self {
        let fetch_state = D::init_state(&mut self.world);
        let filter_state = F::init_state(&mut self.world);

        let mut access = FilteredAccess::default();
        D::update_component_access(&fetch_state, &mut access);

        // Use a temporary empty FilteredAccess for filters. This prevents them from conflicting with the
        // main Query's `fetch_state` access. Filters are allowed to conflict with the main query fetch
        // because they are evaluated *before* a specific reference is constructed.
        let mut filter_access = FilteredAccess::default();
        F::update_component_access(&filter_state, &mut filter_access);

        // Merge the temporary filter access with the main access. This ensures that filter access is
        // properly considered in a global "cross-query" context (both within systems and across systems).
        access.extend(&filter_access);

        self.plan.add_term(access);
        &mut self
    }

    pub fn build(self) -> QueryPlan {
        self.plan
    }
}

#[derive(thiserror::Error)]
pub enum QueryPlanError {
    /// The source does not exist
    #[error("The source with index {0} does not exist")]
    QuerySourceNotFound(u8),
}

/// A dynamic query plan that describes how to match multiple entities
/// connected through relationships.
#[derive(Debug, Default, Clone)]
pub struct QueryPlan {
    /// All variables in this query.
    pub terms: Vec<QueryElement>,
    /// Relationships that connect the terms.
    pub relationships: Vec<QueryRelationship>,
    /// The index of the main term (the one we iterate over).
    pub main_term_index: u8,
}
// TODO: compile step: find a list of ops


// TODO: what of R1(1, 2) and R2(1, 3) ?
//  make ops Query<D, F>(1) and R(1, 2) and R(1, 3)
//  an op is either a query or a R, and both are ordered together.

// next()
// 1. (D, F, 0): use component index and find a list of matching archetypes/tables. Set variable_state to the first row of the first table.
// 2. (R1, 0, 1): get the entity value of 1 via the relationship, set it to written
// 3. (D2, F2, 1): 'written=True' -> check if 1 matches D2, F2
//   If true, keep going to next operation
//   If false, redo the previous operation (but knowing that the next match is false). For example for a FixedMatch, redo means 'False' -> we dont match
//    For a variablematch, redo means go to next row
// 4. (R2, 0, 2): get the entity value of 2 via the relationship


pub struct IterState {
    // index of the source we are currently considering
    pub curr_source: u8,
    // Current index, index + offset = row in table
    pub index: u32,
    // Offset into table
    pub offset: u32,
    // Total entities to iterate in current table
    pub count: u32,

    /// Index of the current entity for each variable
    pub variable_state: Vec<VariableState>,
    /// List of matching tables/archetypes to iterate through for each variable
    pub operation_state: Vec<StorageState>,

    /// Whether we have already found an Entity for the source after processing operation i
    written: Vec<FixedBitSet>,
}


impl QueryPlan {
    /// Create a new empty query plan.
    pub fn new(main_term_index: u8) -> Self {
        Self {
            terms: Vec::new(),
            relationships: Vec::new(),
            main_term_index,
        }
    }

    /// Add a term to the query plan.
    pub(crate) fn add_term(&mut self, access: FilteredAccess) -> usize {
        let term_index = self.terms.len();
        self.terms.push(QueryElement::new(term_index, access));
        term_index
    }

    /// Add a relationship between two terms.
    pub fn add_relationship(
        &mut self,
        source: impl Into<QueryVariable>,
        target: impl Into<QueryVariable>,
        relationship_component: ComponentId,
    ) -> Result<&mut Self, QueryPlanError> {
        let source = source.into();
        let target = target.into();
        let term = self.terms.get_mut(source.index).ok_or(QueryPlanError::QuerySourceNotFound(source.index))?;
        term.relationships.push(QueryRelationship {
            source_term: source.index,
            target_term: target.index,
            relationship_component,
        });
        Ok(self)
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

