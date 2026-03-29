#![cfg(feature = "dynamic_query")]
use crate::relationship::RelationshipAccessor;
use crate::world::unsafe_world_cell::UnsafeWorldCell;
use crate::{
    archetype::Archetype,
    change_detection::Tick,
    component::{ComponentId, Components},
    entity::Entity,
    query::{FilteredAccess, WorldQuery},
    storage::Table,
};
use super::dynamic_plan::{Constraint, DynamicPlan, TermVar, VarId};
use alloc::vec::Vec;
use smallvec::SmallVec;

#[derive(Clone, Debug)]
pub struct DynamicResult {
    pub this: Entity,
    pub vars: SmallVec<[(VarId, Entity); 3]>,
}

#[derive(Clone)]
pub struct DynamicState {
    pub plan: DynamicPlan,
}

#[derive(Clone)]
pub struct DynamicFetch<'w> {
    world: UnsafeWorldCell<'w>,
    components: &'w Components,
    last_run: Tick,
    this_run: Tick,
    current_archetype: Option<&'w Archetype>,
    current_table: Option<&'w Table>,
}

pub struct DynamicData;

unsafe impl WorldQuery for DynamicData {
    type Fetch<'w> = DynamicFetch<'w>;
    type State = DynamicState;

    fn shrink_fetch<'wlong: 'wshort, 'wshort>(fetch: Self::Fetch<'wlong>) -> Self::Fetch<'wshort> {
        DynamicFetch {
            world: fetch.world,
            components: fetch.components,
            last_run: fetch.last_run,
            this_run: fetch.this_run,
            current_archetype: fetch.current_archetype,
            current_table: fetch.current_table,
        }
    }

    unsafe fn init_fetch<'w, 's>(world: UnsafeWorldCell<'w>, _state: &'s Self::State, last_run: Tick, this_run: Tick) -> Self::Fetch<'w> {
        DynamicFetch {
            world,
            components: world.components(),
            last_run,
            this_run,
            current_archetype: None,
            current_table: None,
        }
    }

    const IS_DENSE: bool = false;

    unsafe fn set_archetype<'w, 's>(fetch: &mut Self::Fetch<'w>, _state: &'s Self::State, archetype: &'w Archetype, table: &'w Table) {
        fetch.current_archetype = Some(archetype);
        fetch.current_table = Some(table);
    }

    unsafe fn set_table<'w, 's>(fetch: &mut Self::Fetch<'w>, _state: &'s Self::State, table: &'w Table) {
        fetch.current_archetype = None;
        fetch.current_table = Some(table);
    }

    fn update_component_access(state: &Self::State, access: &mut FilteredAccess) {
        access.extend(&state.plan.filtered_access);
    }

    fn init_state(_world: &mut crate::world::World) -> Self::State {
        panic!("DynamicData::init_state should not be called; build via builder");
    }

    fn get_state(_components: &Components) -> Option<Self::State> { None }

    fn matches_component_set(state: &Self::State, set_contains_id: &impl Fn(ComponentId) -> bool) -> bool {
        state
            .plan
            .filtered_access
            .filter_sets
            .iter()
            .any(|set| {
                set.with
                    .ones()
                    .all(|index| set_contains_id(ComponentId::get_sparse_set_index(index)))
                    && set
                        .without
                        .ones()
                        .all(|index| !set_contains_id(ComponentId::get_sparse_set_index(index)))
            })
    }
}

unsafe impl crate::query::QueryData for DynamicData {
    const IS_READ_ONLY: bool = true;
    type ReadOnly = Self;
    type Item<'w, 's> = DynamicResult;

    fn shrink<'wlong: 'wshort, 'wshort, 's>(item: Self::Item<'wlong, 's>) -> Self::Item<'wshort, 's> { item }

    unsafe fn fetch<'w, 's>(state: &'s <Self as WorldQuery>::State, fetch: &mut <Self as WorldQuery>::Fetch<'w>, entity: Entity, _table_row: crate::storage::TableRow) -> Self::Item<'w, 's> {
        let result = solve_bindings(fetch.world, fetch.components, &state.plan, entity);
        match result {
            Some(vars) => DynamicResult { this: entity, vars },
            None => DynamicResult { this: entity, vars: SmallVec::new() },
        }
    }
}

unsafe impl crate::query::ReadOnlyQueryData for DynamicData {}

fn solve_bindings(world: UnsafeWorldCell, components: &Components, plan: &DynamicPlan, this_entity: Entity) -> Option<SmallVec<[(VarId, Entity); 3]>> {
    let mut bindings: Vec<Option<Entity>> = vec![None; plan.vars.len()];
    bindings[0] = Some(this_entity);

    let mut changed = true;
    let mut passes = 0;
    while changed && passes < plan.constraints.len() + 2 {
        changed = false;
        passes += 1;
        for c in &plan.constraints {
            match *c {
                Constraint::With { var, component, .. } => {
                    if let Some(e) = get_binding(&bindings, var) {
                        // require presence on bound var
                        let cell = world.get_entity(e).ok()?;
                        if !cell.contains_id(component) { return None; }
                    }
                }
                Constraint::Without { var, component } => {
                    if let Some(e) = get_binding(&bindings, var) {
                        let cell = world.get_entity(e).ok()?;
                        if cell.contains_id(component) { return None; }
                    }
                }
                Constraint::Relation { rel, from, to } => {
                    let info = components.get_info(rel)?;
                    let accessor = info.relationship_accessor()?;
                    match (get_binding(&bindings, from), get_binding(&bindings, to), accessor) {
                        (Some(from_e), None, RelationshipAccessor::Relationship { entity_field_offset, .. }) => {
                            // read Relationship on `from`
                            let cell = world.get_entity(from_e).ok()?;
                            // SAFETY: component id valid; offset used below
                            let ptr = unsafe { cell.get_by_id(rel)? };
                            // SAFETY: offset points to Entity field per accessor contract
                            let target: Entity = unsafe { *ptr.byte_add(entity_field_offset).deref() };
                            if set_binding(&mut bindings, to, target) { changed = true; }
                        }
                        (Some(from_e), Some(to_e), RelationshipAccessor::Relationship { entity_field_offset, .. }) => {
                            let cell = world.get_entity(from_e).ok()?;
                            let ptr = unsafe { cell.get_by_id(rel)? };
                            let target: Entity = unsafe { *ptr.byte_add(entity_field_offset).deref() };
                            if target != to_e { return None; }
                        }
                        (None, Some(to_e), RelationshipAccessor::RelationshipTarget { iter, .. }) => {
                            // iterate sources of `to` and bind `from` to first match
                            let cell = world.get_entity(to_e).ok()?;
                            let ptr = unsafe { cell.get_by_id(rel)? };
                            // SAFETY: ptr is of correct component type by id; accessor promises safety
                            let mut it = unsafe { iter(ptr) };
                            if let Some(src) = it.next() {
                                if set_binding(&mut bindings, from, src) { changed = true; }
                            } else {
                                return None;
                            }
                        }
                        (Some(from_e), Some(to_e), RelationshipAccessor::RelationshipTarget { iter, .. }) => {
                            let cell = world.get_entity(to_e).ok()?;
                            let ptr = unsafe { cell.get_by_id(rel)? };
                            let mut it = unsafe { iter(ptr) };
                            if !it.any(|src| src == from_e) { return None; }
                        }
                        // Other combinations (unbound vars) are deferred to later passes
                        _ => {}
                    }
                }
            }
        }
    }

    // Final validation: all With/Without on bound vars satisfied (already checked), ensure any still-unbound var mentioned in a With must fail
    for c in &plan.constraints {
        match *c {
            Constraint::With { var, .. } | Constraint::Without { var, .. } => {
                if get_binding(&bindings, var).is_none() { return None; }
            }
            _ => {}
        }
    }

    let mut out: SmallVec<[(VarId, Entity); 3]> = SmallVec::new();
    for (idx, b) in bindings.into_iter().enumerate() {
        if let Some(e) = b { if idx != 0 { out.push((VarId(idx as u32), e)); } }
    }
    Some(out)
}

fn get_binding(bindings: &Vec<Option<Entity>>, var: TermVar) -> Option<Entity> {
    match var { TermVar::This => bindings[0], TermVar::Var(VarId(i)) => bindings.get(i as usize).and_then(|b| *b) }
}

fn set_binding(bindings: &mut Vec<Option<Entity>>, var: TermVar, value: Entity) -> bool {
    match var {
        TermVar::This => {
            if let Some(prev) = bindings[0] { prev == value } else { bindings[0] = Some(value); true }
        }
        TermVar::Var(VarId(i)) => {
            let slot = &mut bindings[i as usize];
            if let Some(prev) = *slot { prev == value } else { *slot = Some(value); true }
        }
    }
}


