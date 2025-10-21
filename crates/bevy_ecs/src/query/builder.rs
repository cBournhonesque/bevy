use core::marker::PhantomData;

use crate::{
    component::{ComponentId, StorageType},
    prelude::*,
};

use super::{FilteredAccess, QueryData, QueryFilter};
#[cfg(feature = "dynamic_query")]
use super::dynamic_plan::*;
#[cfg(feature = "dynamic_query")]
use alloc::vec::Vec;
#[cfg(feature = "dynamic_query")]
use smallvec::SmallVec;
#[cfg(feature = "dynamic_query")]
use crate::component::ComponentId;
#[cfg(feature = "dynamic_query")]
use crate::world::FilteredEntityRef;
#[cfg(feature = "dynamic_query")]
use crate::query::state::QueryState;

/// Builder struct to create [`QueryState`] instances at runtime.
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
/// # #[derive(Component)]
/// # struct C;
/// #
/// let mut world = World::new();
/// let entity_a = world.spawn((A, B)).id();
/// let entity_b = world.spawn((A, C)).id();
///
/// // Instantiate the builder using the type signature of the iterator you will consume
/// let mut query = QueryBuilder::<(Entity, &B)>::new(&mut world)
/// // Add additional terms through builder methods
///     .with::<A>()
///     .without::<C>()
///     .build();
///
/// // Consume the QueryState
/// let (entity, b) = query.single(&world).unwrap();
/// ```
pub struct QueryBuilder<'w, D: QueryData = (), F: QueryFilter = ()> {
    access: FilteredAccess,
    world: &'w mut World,
    or: bool,
    first: bool,
    _marker: PhantomData<(D, F)>,
    #[cfg(feature = "dynamic_query")]
    plan: DynamicPlan,
}

impl<'w, D: QueryData, F: QueryFilter> QueryBuilder<'w, D, F> {
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
            #[cfg(feature = "dynamic_query")]
            plan: DynamicPlan::new(),
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

    /// Takes a function over mutable access to a [`QueryBuilder`], calls that function
    /// on an empty builder and then adds all accesses from that builder to self as optional.
    pub fn optional(&mut self, f: impl Fn(&mut QueryBuilder)) -> &mut Self {
        let mut builder = QueryBuilder::new(self.world);
        f(&mut builder);
        self.access.extend_access(builder.access());
        self
    }

    /// Takes a function over mutable access to a [`QueryBuilder`], calls that function
    /// on an empty builder and then adds all accesses from that builder to self.
    ///
    /// Primarily used when inside a [`Self::or`] closure to group several terms.
    pub fn and(&mut self, f: impl Fn(&mut QueryBuilder)) -> &mut Self {
        let mut builder = QueryBuilder::new(self.world);
        f(&mut builder);
        let access = builder.access().clone();
        self.extend_access(access);
        self
    }

    /// Takes a function over mutable access to a [`QueryBuilder`], calls that function
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
    pub fn or(&mut self, f: impl Fn(&mut QueryBuilder)) -> &mut Self {
        let mut builder = QueryBuilder::new(self.world);
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
    pub fn transmute<NewD: QueryData>(&mut self) -> &mut QueryBuilder<'w, NewD> {
        self.transmute_filtered::<NewD, ()>()
    }

    /// Transmute the existing builder adding required accesses.
    /// This will maintain all existing accesses.
    pub fn transmute_filtered<NewD: QueryData, NewF: QueryFilter>(
        &mut self,
    ) -> &mut QueryBuilder<'w, NewD, NewF> {
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

    // ===== Dynamic extensions (behind feature) =====
    #[cfg(feature = "dynamic_query")]
    /// Create a new variable/term and return its id.
    pub fn var(&mut self) -> VarId { self.plan.var() }

    #[cfg(feature = "dynamic_query")]
    /// Constrain that `var` has component `id` (read).
    pub fn with_id_var(&mut self, id: ComponentId, var: TermVar) -> &mut Self {
        // Update global filtered access like a normal With + read
        let mut access = FilteredAccess::default();
        access.and_with(id);
        access.add_component_read(id);
        self.extend_access(access);
        // Record constraint
        self.plan.constraints.push(Constraint::With { var, component: id, write: false });
        self
    }

    #[cfg(feature = "dynamic_query")]
    /// Constrain that `var` has component `id` (write).
    pub fn mut_id_var(&mut self, id: ComponentId, var: TermVar) -> &mut Self {
        let mut access = FilteredAccess::default();
        access.and_with(id);
        access.add_component_write(id);
        self.extend_access(access);
        self.plan.constraints.push(Constraint::With { var, component: id, write: true });
        self
    }

    #[cfg(feature = "dynamic_query")]
    /// Constrain that `var` does not have component `id`.
    pub fn without_id_var(&mut self, id: ComponentId, var: TermVar) -> &mut Self {
        let mut access = FilteredAccess::default();
        access.and_without(id);
        self.extend_access(access);
        self.plan.constraints.push(Constraint::Without { var, component: id });
        self
    }

    #[cfg(feature = "dynamic_query")]
    /// Constrain a relation between two variables.
    pub fn relation_id(&mut self, rel: ComponentId, from: TermVar, to: TermVar) -> &mut Self {
        // Relations may be stored sparsely; for now mark via a With on the `from` side so archetype pruning can work when possible.
        let mut access = FilteredAccess::default();
        access.and_with(rel);
        self.extend_access(access);
        self.plan.constraints.push(Constraint::Relation { rel, from, to });
        self
    }

    #[cfg(feature = "dynamic_query")]
    /// Build a dynamic query that matches using the accumulated plan.
    pub fn build_dynamic(&mut self) -> QueryState<crate::query::DynamicData> {
        use crate::query::{DynamicData, DynamicState, QueryState};
        let world = self.world();
        // pessimistic dense
        let is_dense = false;
        let fetch_state = DynamicState { plan: self.plan.clone() };
        let filter_state = (); // no filter
        QueryState::<DynamicData>::from_states_uninitialized_with_access(
            world,
            fetch_state,
            filter_state,
            self.access.clone(),
            is_dense,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        prelude::*,
        world::{EntityMutExcept, EntityRefExcept, FilteredEntityMut, FilteredEntityRef},
    };
    use std::dbg;

    #[derive(Component, PartialEq, Debug)]
    struct A(usize);

    #[derive(Component, PartialEq, Debug)]
    struct B(usize);

    #[derive(Component, PartialEq, Debug)]
    struct C(usize);

    #[test]
    fn builder_with_without_static() {
        let mut world = World::new();
        let entity_a = world.spawn((A(0), B(0))).id();
        let entity_b = world.spawn((A(0), C(0))).id();

        let mut query_a = QueryBuilder::<Entity>::new(&mut world)
            .with::<A>()
            .without::<C>()
            .build();
        assert_eq!(entity_a, query_a.single(&world).unwrap());

        let mut query_b = QueryBuilder::<Entity>::new(&mut world)
            .with::<A>()
            .without::<B>()
            .build();
        assert_eq!(entity_b, query_b.single(&world).unwrap());
    }

    #[test]
    fn builder_with_without_dynamic() {
        let mut world = World::new();
        let entity_a = world.spawn((A(0), B(0))).id();
        let entity_b = world.spawn((A(0), C(0))).id();
        let component_id_a = world.register_component::<A>();
        let component_id_b = world.register_component::<B>();
        let component_id_c = world.register_component::<C>();

        let mut query_a = QueryBuilder::<Entity>::new(&mut world)
            .with_id(component_id_a)
            .without_id(component_id_c)
            .build();
        assert_eq!(entity_a, query_a.single(&world).unwrap());

        let mut query_b = QueryBuilder::<Entity>::new(&mut world)
            .with_id(component_id_a)
            .without_id(component_id_b)
            .build();
        assert_eq!(entity_b, query_b.single(&world).unwrap());
    }

    #[test]
    fn builder_or() {
        let mut world = World::new();
        world.spawn((A(0), B(0)));
        world.spawn(B(0));
        world.spawn(C(0));

        let mut query_a = QueryBuilder::<Entity>::new(&mut world)
            .or(|builder| {
                builder.with::<A>();
                builder.with::<B>();
            })
            .build();
        assert_eq!(2, query_a.iter(&world).count());

        let mut query_b = QueryBuilder::<Entity>::new(&mut world)
            .or(|builder| {
                builder.with::<A>();
                builder.without::<B>();
            })
            .build();
        dbg!(&query_b.component_access);
        assert_eq!(2, query_b.iter(&world).count());

        let mut query_c = QueryBuilder::<Entity>::new(&mut world)
            .or(|builder| {
                builder.with::<A>();
                builder.with::<B>();
                builder.with::<C>();
            })
            .build();
        assert_eq!(3, query_c.iter(&world).count());
    }

    #[test]
    fn builder_transmute() {
        let mut world = World::new();
        world.spawn(A(0));
        world.spawn((A(1), B(0)));
        let mut query = QueryBuilder::<()>::new(&mut world)
            .with::<B>()
            .transmute::<&A>()
            .build();

        query.iter(&world).for_each(|a| assert_eq!(a.0, 1));
    }

    #[test]
    fn builder_static_components() {
        let mut world = World::new();
        let entity = world.spawn((A(0), B(1))).id();

        let mut query = QueryBuilder::<FilteredEntityRef>::new(&mut world)
            .data::<&A>()
            .data::<&B>()
            .build();

        let entity_ref = query.single(&world).unwrap();

        assert_eq!(entity, entity_ref.id());

        let a = entity_ref.get::<A>().unwrap();
        let b = entity_ref.get::<B>().unwrap();

        assert_eq!(0, a.0);
        assert_eq!(1, b.0);
    }

    #[test]
    fn builder_dynamic_components() {
        let mut world = World::new();
        let entity = world.spawn((A(0), B(1))).id();
        let component_id_a = world.register_component::<A>();
        let component_id_b = world.register_component::<B>();

        let mut query = QueryBuilder::<FilteredEntityRef>::new(&mut world)
            .ref_id(component_id_a)
            .ref_id(component_id_b)
            .build();

        let entity_ref = query.single(&world).unwrap();

        assert_eq!(entity, entity_ref.id());

        let a = entity_ref.get_by_id(component_id_a).unwrap();
        let b = entity_ref.get_by_id(component_id_b).unwrap();

        // SAFETY: We set these pointers to point to these components
        unsafe {
            assert_eq!(0, a.deref::<A>().0);
            assert_eq!(1, b.deref::<B>().0);
        }
    }

    #[test]
    fn builder_provide_access() {
        let mut world = World::new();
        world.spawn((A(0), B(1)));

        let mut query =
            QueryBuilder::<(Entity, FilteredEntityRef, FilteredEntityMut)>::new(&mut world)
                .data::<&mut A>()
                .data::<&B>()
                .build();

        // The `FilteredEntityRef` only has read access, so the `FilteredEntityMut` can have read access without conflicts
        let (_entity, entity_ref_1, mut entity_ref_2) = query.single_mut(&mut world).unwrap();
        assert!(entity_ref_1.get::<A>().is_some());
        assert!(entity_ref_1.get::<B>().is_some());
        assert!(entity_ref_2.get::<A>().is_some());
        assert!(entity_ref_2.get_mut::<A>().is_none());
        assert!(entity_ref_2.get::<B>().is_some());
        assert!(entity_ref_2.get_mut::<B>().is_none());

        let mut query =
            QueryBuilder::<(Entity, FilteredEntityMut, FilteredEntityMut)>::new(&mut world)
                .data::<&mut A>()
                .data::<&B>()
                .build();

        // The first `FilteredEntityMut` has write access to A, so the second one cannot have write access
        let (_entity, mut entity_ref_1, mut entity_ref_2) = query.single_mut(&mut world).unwrap();
        assert!(entity_ref_1.get::<A>().is_some());
        assert!(entity_ref_1.get_mut::<A>().is_some());
        assert!(entity_ref_1.get::<B>().is_some());
        assert!(entity_ref_1.get_mut::<B>().is_none());
        assert!(entity_ref_2.get::<A>().is_none());
        assert!(entity_ref_2.get_mut::<A>().is_none());
        assert!(entity_ref_2.get::<B>().is_some());
        assert!(entity_ref_2.get_mut::<B>().is_none());

        let mut query = QueryBuilder::<(FilteredEntityMut, &mut A, &B)>::new(&mut world)
            .data::<&mut A>()
            .data::<&mut B>()
            .build();

        // Any `A` access would conflict with `&mut A`, and write access to `B` would conflict with `&B`.
        let (mut entity_ref, _a, _b) = query.single_mut(&mut world).unwrap();
        assert!(entity_ref.get::<A>().is_none());
        assert!(entity_ref.get_mut::<A>().is_none());
        assert!(entity_ref.get::<B>().is_some());
        assert!(entity_ref.get_mut::<B>().is_none());

        let mut query = QueryBuilder::<(FilteredEntityMut, &mut A, &B)>::new(&mut world)
            .data::<EntityMut>()
            .build();

        // Same as above, but starting from "all" access
        let (mut entity_ref, _a, _b) = query.single_mut(&mut world).unwrap();
        assert!(entity_ref.get::<A>().is_none());
        assert!(entity_ref.get_mut::<A>().is_none());
        assert!(entity_ref.get::<B>().is_some());
        assert!(entity_ref.get_mut::<B>().is_none());

        let mut query = QueryBuilder::<(FilteredEntityMut, EntityMutExcept<A>)>::new(&mut world)
            .data::<EntityMut>()
            .build();

        // Removing `EntityMutExcept<A>` just leaves A
        let (mut entity_ref_1, _entity_ref_2) = query.single_mut(&mut world).unwrap();
        assert!(entity_ref_1.get::<A>().is_some());
        assert!(entity_ref_1.get_mut::<A>().is_some());
        assert!(entity_ref_1.get::<B>().is_none());
        assert!(entity_ref_1.get_mut::<B>().is_none());

        let mut query = QueryBuilder::<(FilteredEntityMut, EntityRefExcept<A>)>::new(&mut world)
            .data::<EntityMut>()
            .build();

        // Removing `EntityRefExcept<A>` just leaves A, plus read access
        let (mut entity_ref_1, _entity_ref_2) = query.single_mut(&mut world).unwrap();
        assert!(entity_ref_1.get::<A>().is_some());
        assert!(entity_ref_1.get_mut::<A>().is_some());
        assert!(entity_ref_1.get::<B>().is_some());
        assert!(entity_ref_1.get_mut::<B>().is_none());
    }

    /// Regression test for issue #14348
    #[test]
    fn builder_static_dense_dynamic_sparse() {
        #[derive(Component)]
        struct Dense;

        #[derive(Component)]
        #[component(storage = "SparseSet")]
        struct Sparse;

        let mut world = World::new();

        world.spawn(Dense);
        world.spawn((Dense, Sparse));

        let mut query = QueryBuilder::<&Dense>::new(&mut world)
            .with::<Sparse>()
            .build();

        let matched = query.iter(&world).count();
        assert_eq!(matched, 1);
    }
}
