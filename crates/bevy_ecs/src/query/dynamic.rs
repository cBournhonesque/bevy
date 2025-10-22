use crate::{
    archetype::Archetype,
    component::{ComponentId, Components, Tick},
    entity::Entity,
    query::{
        Access, FilteredAccess, FilteredAccessSet, QueryData, QueryPlan, ReadOnlyQueryData,
        WorldQuery,
    },
    storage::Table,
    world::{unsafe_world_cell::UnsafeWorldCell, FilteredEntityRef, World},
};
use alloc::vec::Vec;

/// A dynamic query that uses a [`QueryPlan`] to match multiple entities
/// connected through relationships.
///
/// This query type fetches all matching entity combinations based on the plan,
/// providing [`FilteredEntityRef`] access to each entity in the result.
///
/// # Overview
///
/// `Dynamic` enables querying multiple related entities in a single query.
/// Unlike standard Bevy queries that only access the current entity being iterated,
/// `Dynamic` can follow relationships to access data from connected entities.
///
/// # Example: Finding Spaceships and their Factions
///
/// ```rust
/// # use bevy_ecs::prelude::*;
/// # use bevy_ecs::query::{TypedQueryPlanBuilder, DynamicState};
/// # use bevy_ecs::relationship::Relationship;
/// #
/// # #[derive(Component)]
/// # struct SpaceShip { name: String }
/// #
/// # #[derive(Component)]
/// # struct Faction(Entity);
/// #
/// # impl Relationship for Faction {
/// #     type RelationshipTarget = ();
/// #     fn get(&self) -> Entity { self.0 }
/// #     fn from(e: Entity) -> Self { Faction(e) }
/// #     fn set_risky(&mut self, e: Entity) { self.0 = e; }
/// # }
/// #
/// # #[derive(Component)]
/// # struct FactionName { name: String }
/// #
/// # let mut world = World::new();
/// #
/// // Build a query: SpaceShip -> Faction (entity)
/// let mut builder = TypedQueryPlanBuilder::new(&mut world);
/// let spaceship_term = builder.with::<SpaceShip>();  // Term 0
/// let faction_term = builder.with::<FactionName>();   // Term 1
/// builder.related_to::<Faction>(spaceship_term, faction_term);
///
/// let plan = builder.build(spaceship_term);
///
/// // Create the state that would be used in a QueryState<Dynamic>
/// let state = DynamicState::from_plan(plan);
///
/// // In a system, you would then iterate and access matched entities:
/// // for item in query.iter() {
/// //     for dynamic_match in item.iter() {
/// //         let spaceship_entity = dynamic_match.entity(0);
/// //         let faction_entity = dynamic_match.entity(1);
/// //         // Access components...
/// //     }
/// // }
/// ```
///
/// # Complex Example: Multi-hop Relationships
///
/// ```rust
/// # use bevy_ecs::prelude::*;
/// # use bevy_ecs::query::TypedQueryPlanBuilder;
/// # use bevy_ecs::relationship::Relationship;
/// #
/// # #[derive(Component)] struct SpaceShip;
/// # #[derive(Component)] struct Faction(Entity);
/// # #[derive(Component)] struct DockedTo(Entity);
/// # #[derive(Component)] struct Planet;
/// # #[derive(Component)] struct RuledBy(Entity);
/// # #[derive(Component)] struct AlliedWith(Entity);
/// #
/// # impl Relationship for Faction {
/// #     type RelationshipTarget = ();
/// #     fn get(&self) -> Entity { self.0 }
/// #     fn from(e: Entity) -> Self { Faction(e) }
/// #     fn set_risky(&mut self, e: Entity) { self.0 = e; }
/// # }
/// # impl Relationship for DockedTo {
/// #     type RelationshipTarget = ();
/// #     fn get(&self) -> Entity { self.0 }
/// #     fn from(e: Entity) -> Self { DockedTo(e) }
/// #     fn set_risky(&mut self, e: Entity) { self.0 = e; }
/// # }
/// # impl Relationship for RuledBy {
/// #     type RelationshipTarget = ();
/// #     fn get(&self) -> Entity { self.0 }
/// #     fn from(e: Entity) -> Self { RuledBy(e) }
/// #     fn set_risky(&mut self, e: Entity) { self.0 = e; }
/// # }
/// # impl Relationship for AlliedWith {
/// #     type RelationshipTarget = ();
/// #     fn get(&self) -> Entity { self.0 }
/// #     fn from(e: Entity) -> Self { AlliedWith(e) }
/// #     fn set_risky(&mut self, e: Entity) { self.0 = e; }
/// # }
/// #
/// # let mut world = World::new();
/// // SpaceShip -> Faction -> AlliedWith
/// //          \-> DockedTo -> Planet -> RuledBy
/// let mut builder = TypedQueryPlanBuilder::new(&mut world);
///
/// let spaceship = builder.with::<SpaceShip>();           // 0
/// let spaceship_faction = builder.term();                 // 1
/// let planet = builder.with::<Planet>();                  // 2
/// let planet_faction = builder.term();                    // 3
///
/// builder.related_to::<Faction>(spaceship, spaceship_faction);
/// builder.related_to::<DockedTo>(spaceship, planet);
/// builder.related_to::<RuledBy>(planet, planet_faction);
/// builder.related_to::<AlliedWith>(spaceship_faction, planet_faction);
///
/// let plan = builder.build(spaceship);
/// // This plan finds spaceships docked at planets where the spaceship's faction
/// // is allied with the planet's ruling faction
/// ```
pub struct Dynamic {
    _private: (),
}

/// The state for a [`Dynamic`] query, containing the query plan.
pub struct DynamicState {
    /// The query plan defining how entities are related.
    pub plan: QueryPlan,
}

/// The fetch type for [`Dynamic`] queries.
#[derive(Clone)]
pub struct DynamicFetch<'w> {
    world: UnsafeWorldCell<'w>,
    /// Current entity being processed (from the main term).
    current_entity: Option<Entity>,
}

/// A single match result from a dynamic query.
///
/// Contains one entity for each term in the query plan.
pub struct DynamicMatch<'w, 's> {
    /// The entities that matched, one per term in the query plan.
    pub entities: Vec<Entity>,
    /// Reference to the world for accessing component data.
    world: UnsafeWorldCell<'w>,
    /// Reference to the plan state for accessing per-term access info.
    state: &'s DynamicState,
}

impl<'w, 's> DynamicMatch<'w, 's> {
    /// Get a [`FilteredEntityRef`] for the entity at the given term index.
    ///
    /// # Safety
    /// - `term_index` must be valid (< plan.terms.len())
    /// - The caller must ensure proper access synchronization
    pub unsafe fn entity_ref(&self, term_index: usize) -> FilteredEntityRef<'w, 's> {
        let entity = self.entities[term_index];
        let access = &self.state.plan.terms[term_index].access.access();

        // Get entity location
        let location = self.world.entities().get(entity).unwrap();
        let last_change_tick = self.world.last_change_tick();
        let change_tick = self.world.change_tick();

        let cell = crate::world::unsafe_world_cell::UnsafeEntityCell::new(
            self.world,
            entity,
            location,
            last_change_tick,
            change_tick,
        );

        // SAFETY: Access is properly defined in the query plan
        FilteredEntityRef::new(cell, access)
    }

    /// Get the entity ID for a specific term.
    pub fn entity(&self, term_index: usize) -> Entity {
        self.entities[term_index]
    }

    /// Get a component from the entity at the given term index.
    ///
    /// Returns `None` if the entity doesn't have the component or if
    /// the term's access doesn't include read access to this component.
    ///
    /// # Safety
    /// - `term_index` must be valid (< plan.terms.len())
    /// - The caller must ensure proper access synchronization
    pub unsafe fn get<T: crate::component::Component>(&self, term_index: usize) -> Option<&'w T> {
        let entity_ref = self.entity_ref(term_index);
        entity_ref.get::<T>()
    }

    /// Get all entities in this match.
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    /// Get the number of terms in this match.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Returns true if there are no terms.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
}

/// The query item returned by [`Dynamic`].
///
/// Contains all matching entity combinations for the current main entity.
pub struct DynamicItem<'w, 's> {
    /// All entity combinations that matched the query plan.
    pub matches: Vec<DynamicMatch<'w, 's>>,
}

impl<'w, 's> DynamicItem<'w, 's> {
    /// Iterate over all matches.
    pub fn iter(&self) -> impl Iterator<Item = &DynamicMatch<'w, 's>> {
        self.matches.iter()
    }

    /// Get the number of matches.
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// Returns true if there are no matches.
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }
}

// SAFETY: DynamicFetch only accesses entities according to the plan's access
unsafe impl WorldQuery for Dynamic {
    type Fetch<'w> = DynamicFetch<'w>;
    type State = DynamicState;

    fn shrink_fetch<'wlong: 'wshort, 'wshort>(
        fetch: Self::Fetch<'wlong>,
    ) -> Self::Fetch<'wshort> {
        fetch
    }

    #[inline]
    unsafe fn init_fetch<'w, 's>(
        world: UnsafeWorldCell<'w>,
        _state: &'s Self::State,
        _last_run: Tick,
        _this_run: Tick,
    ) -> Self::Fetch<'w> {
        DynamicFetch {
            world,
            current_entity: None,
        }
    }

    // Dynamic queries are not dense since they need entity-level matching
    const IS_DENSE: bool = false;

    #[inline]
    unsafe fn set_archetype<'w, 's>(
        _fetch: &mut Self::Fetch<'w>,
        _state: &'s Self::State,
        _archetype: &'w Archetype,
        _table: &'w Table,
    ) {
        // Nothing to set - we process entities individually
    }

    #[inline]
    unsafe fn set_table<'w, 's>(
        _fetch: &mut Self::Fetch<'w>,
        _state: &'s Self::State,
        _table: &'w Table,
    ) {
        // Nothing to set - we process entities individually
    }

    fn update_component_access(state: &Self::State, access: &mut FilteredAccess) {
        // Add the main term's access for archetype matching
        access.extend(state.plan.main_term_access());
    }

    fn init_nested_access(
        state: &Self::State,
        _system_name: Option<&str>,
        component_access_set: &mut FilteredAccessSet,
        _world: UnsafeWorldCell,
    ) {
        // Add access for all non-main terms since they access other entities
        for (i, term) in state.plan.terms.iter().enumerate() {
            if i != state.plan.main_term_index {
                component_access_set.add(term.access.clone());
            }
        }
    }

    fn init_state(_world: &mut World) -> Self::State {
        // Create an empty plan - this would typically be set by the builder
        DynamicState {
            plan: QueryPlan::new(0),
        }
    }

    fn get_state(_components: &Components) -> Option<Self::State> {
        // For now, return a default empty state
        // In a real implementation, this would need the plan from somewhere
        Some(DynamicState {
            plan: QueryPlan::new(0),
        })
    }

    fn matches_component_set(
        state: &Self::State,
        set_contains_id: &impl Fn(ComponentId) -> bool,
    ) -> bool {
        // Check if the main term matches
        let main_term = &state.plan.terms[state.plan.main_term_index];

        // Check all required components from the main term's access
        if let Ok(components) = main_term.access.access().try_iter_component_access() {
            for component_access in components {
                let component_id = match component_access {
                    crate::query::ComponentAccessKind::Exclusive(id) => id,
                    crate::query::ComponentAccessKind::Shared(id) => id,
                    crate::query::ComponentAccessKind::Archetypal(id) => id,
                };
                if !set_contains_id(component_id) {
                    return false;
                }
            }
        }

        true
    }
}

// SAFETY: Dynamic only provides read-only access via FilteredEntityRef
unsafe impl QueryData for Dynamic {
    const IS_READ_ONLY: bool = true;

    type ReadOnly = Self;
    type Item<'w, 's> = DynamicItem<'w, 's>;

    fn shrink<'wlong: 'wshort, 'wshort, 's>(
        item: Self::Item<'wlong, 's>,
    ) -> Self::Item<'wshort, 's> {
        item
    }

    fn provide_extra_access(
        _state: &mut Self::State,
        _access: &mut Access,
        _available_access: &Access,
    ) {
        // No extra access needed for now
    }

    #[inline]
    unsafe fn fetch<'w, 's>(
        state: &'s Self::State,
        fetch: &mut Self::Fetch<'w>,
        entity: Entity,
        _table_row: crate::storage::TableRow,
    ) -> Self::Item<'w, 's> {
        // Store current entity
        fetch.current_entity = Some(entity);

        // Execute the query plan for this entity
        let results = state.plan.execute(entity, fetch.world);

        // Convert results to DynamicMatch objects
        let matches = results
            .into_iter()
            .map(|entities| DynamicMatch {
                entities,
                world: fetch.world,
                state,
            })
            .collect();

        DynamicItem { matches }
    }
}

// SAFETY: Dynamic can be iterated
unsafe impl crate::query::IterQueryData for Dynamic {}

// SAFETY: DynamicFetch only provides read-only access
unsafe impl ReadOnlyQueryData for Dynamic {}

impl DynamicState {
    /// Create a new dynamic query state from a query plan.
    pub fn from_plan(plan: QueryPlan) -> Self {
        Self { plan }
    }

    /// Get a reference to the underlying query plan.
    pub fn plan(&self) -> &QueryPlan {
        &self.plan
    }

    /// Get a mutable reference to the underlying query plan.
    pub fn plan_mut(&mut self) -> &mut QueryPlan {
        &mut self.plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        component::Component,
        hierarchy::ChildOf,
        query::{QueryPlanBuilder, RelationshipAccessor, TypedQueryPlanBuilder},
        world::World,
    };

    #[derive(Component)]
    struct TestMarker;

    #[derive(Component, Debug, PartialEq)]
    struct Health(u32);

    #[test]
    fn test_dynamic_state_creation() {
        let mut world = World::new();
        let marker_id = world.register_component::<TestMarker>();

        let mut builder = QueryPlanBuilder::new();
        let mut access = FilteredAccess::matches_everything();
        access.add_component_read(marker_id);
        let term = builder.add_term(access);
        let plan = builder.build(term);

        let state = DynamicState::from_plan(plan);
        assert_eq!(state.plan.terms.len(), 1);
    }

    #[test]
    fn test_dynamic_match() {
        let mut world = World::new();

        // Create some entities
        let parent = world.spawn_empty().id();
        let child = world.spawn((TestMarker, ChildOf(parent))).id();
        world.flush();

        let marker_id = world.register_component::<TestMarker>();
        let child_of_id = world.register_component::<ChildOf>();

        // Build plan
        let mut builder = QueryPlanBuilder::new();
        let mut access0 = FilteredAccess::matches_everything();
        access0.add_component_read(marker_id);
        let term0 = builder.add_term(access0);

        let access1 = FilteredAccess::matches_everything();
        let term1 = builder.add_term(access1);

        use core::mem::offset_of;
        builder.add_relationship(
            term0,
            term1,
            child_of_id,
            RelationshipAccessor::Relationship {
                entity_field_offset: offset_of!(ChildOf, 0),
                linked_spawn: true,
            },
        );

        let plan = builder.build(term0);

        // Execute plan directly
        unsafe {
            let world_cell = world.as_unsafe_world_cell_readonly();
            let results = plan.execute(child, world_cell);

            assert_eq!(results.len(), 1);
            assert_eq!(results[0].len(), 2);
            assert_eq!(results[0][0], child);
            assert_eq!(results[0][1], parent);
        }
    }

    #[test]
    fn test_dynamic_match_component_access() {
        let mut world = World::new();

        // Create entities with health
        let parent = world.spawn(Health(100)).id();
        let child = world.spawn((Health(50), ChildOf(parent))).id();
        world.flush();

        let health_id = world.register_component::<Health>();
        let child_of_id = world.register_component::<ChildOf>();

        // Build plan that accesses Health on both terms
        let mut builder = QueryPlanBuilder::new();
        let mut access0 = FilteredAccess::matches_everything();
        access0.add_component_read(health_id);
        let term0 = builder.add_term(access0);

        let mut access1 = FilteredAccess::matches_everything();
        access1.add_component_read(health_id);
        let term1 = builder.add_term(access1);

        use core::mem::offset_of;
        builder.add_relationship(
            term0,
            term1,
            child_of_id,
            RelationshipAccessor::Relationship {
                entity_field_offset: offset_of!(ChildOf, 0),
                linked_spawn: true,
            },
        );

        let plan = builder.build(term0);

        // Execute and access components
        unsafe {
            let world_cell = world.as_unsafe_world_cell_readonly();
            let results = plan.execute(child, world_cell);

            assert_eq!(results.len(), 1);
            let match_result = &results[0];

            // Create a DynamicMatch to test component access
            let dynamic_match = DynamicMatch {
                entities: match_result.clone(),
                world: world_cell,
                state: &DynamicState::from_plan(plan),
            };

            // Access Health component on child (term 0)
            let child_health = dynamic_match.get::<Health>(0);
            assert_eq!(child_health, Some(&Health(50)));

            // Access Health component on parent (term 1)
            let parent_health = dynamic_match.get::<Health>(1);
            assert_eq!(parent_health, Some(&Health(100)));
        }
    }

    #[test]
    fn test_typed_builder_with_dynamic() {
        let mut world = World::new();

        // Use the typed builder API
        let mut builder = TypedQueryPlanBuilder::new(&mut world);
        let child_term = builder.with::<Health>();
        let parent_term = builder.with::<Health>();
        builder.related_to::<ChildOf>(child_term, parent_term);

        let plan = builder.build(child_term);

        // Create test entities
        let parent = world.spawn(Health(100)).id();
        let child = world.spawn((Health(50), ChildOf(parent))).id();
        world.flush();

        // Execute the plan
        unsafe {
            let world_cell = world.as_unsafe_world_cell_readonly();
            let results = plan.execute(child, world_cell);

            assert_eq!(results.len(), 1);
            assert_eq!(results[0][0], child);
            assert_eq!(results[0][1], parent);
        }
    }
}

