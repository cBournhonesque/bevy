// This file contains comprehensive examples of using the Dynamic query system.
// These are meant to be illustrative and would be actual tests once integrated.

#![allow(dead_code, unused_imports, unused_variables)]

use crate::{
    component::Component,
    entity::Entity,
    query::{DynamicState, QueryPlan, TypedQueryPlanBuilder},
    relationship::Relationship,
    world::World,
};

// ============================================================================
// EXAMPLE 1: Simple Parent-Child Health Query
// ============================================================================

#[derive(Component)]
struct Health(u32);

fn example_parent_child_health() {
    use crate::hierarchy::ChildOf;

    let mut world = World::new();

    // Create entities
    let parent = world.spawn(Health(100)).id();
    let child1 = world.spawn((Health(50), ChildOf(parent))).id();
    let child2 = world.spawn((Health(75), ChildOf(parent))).id();
    world.flush();

    // Build query plan: Find children with health -> their parents (also with health)
    let mut builder = TypedQueryPlanBuilder::new(&mut world);
    let child_term = builder.with::<Health>();      // Term 0: child with Health
    let parent_term = builder.with::<Health>();      // Term 1: parent with Health
    builder.related_to::<ChildOf>(child_term, parent_term);

    let plan = builder.build(child_term);

    // Execute for each child
    for &child in &[child1, child2] {
        unsafe {
            let world_cell = world.as_unsafe_world_cell_readonly();
            let results = plan.execute(child, world_cell);

            for entities in &results {
                println!("Child: {:?}, Parent: {:?}", entities[0], entities[1]);
                // In a real system, you would access components here:
                // let child_health = dynamic_match.get::<Health>(0);
                // let parent_health = dynamic_match.get::<Health>(1);
            }
        }
    }
}

// ============================================================================
// EXAMPLE 2: Space Game Scenario (from plan.md)
// ============================================================================

#[derive(Component)]
struct SpaceShip {
    name: &'static str,
}

#[derive(Component)]
struct Planet {
    name: &'static str,
}

#[derive(Component)]
struct FactionTag;

// Relationship: SpaceShip belongs to a Faction
#[derive(Component)]
struct BelongsToFaction(Entity);

impl Relationship for BelongsToFaction {
    type RelationshipTarget = ();

    fn get(&self) -> Entity {
        self.0
    }

    fn from(entity: Entity) -> Self {
        BelongsToFaction(entity)
    }

    fn set_risky(&mut self, entity: Entity) {
        self.0 = entity;
    }
}

// Relationship: SpaceShip is docked to a Planet
#[derive(Component)]
struct DockedTo(Entity);

impl Relationship for DockedTo {
    type RelationshipTarget = ();

    fn get(&self) -> Entity {
        self.0
    }

    fn from(entity: Entity) -> Self {
        DockedTo(entity)
    }

    fn set_risky(&mut self, entity: Entity) {
        self.0 = entity;
    }
}

// Relationship: Planet is ruled by a Faction
#[derive(Component)]
struct RuledBy(Entity);

impl Relationship for RuledBy {
    type RelationshipTarget = ();

    fn get(&self) -> Entity {
        self.0
    }

    fn from(entity: Entity) -> Self {
        RuledBy(entity)
    }

    fn set_risky(&mut self, entity: Entity) {
        self.0 = entity;
    }
}

// Relationship: Faction is allied with another Faction
#[derive(Component)]
struct AlliedWith(Entity);

impl Relationship for AlliedWith {
    type RelationshipTarget = ();

    fn get(&self) -> Entity {
        self.0
    }

    fn from(entity: Entity) -> Self {
        AlliedWith(entity)
    }

    fn set_risky(&mut self, entity: Entity) {
        self.0 = entity;
    }
}

fn example_space_game_complex_query() {
    let mut world = World::new();

    // Create factions
    let faction_a = world.spawn(FactionTag).id();
    let faction_b = world.spawn(FactionTag).id();
    world.entity_mut(faction_a).insert(AlliedWith(faction_b));

    // Create a planet ruled by faction B
    let planet = world.spawn(Planet { name: "Earth" }).id();
    world.entity_mut(planet).insert(RuledBy(faction_b));

    // Create a spaceship belonging to faction A, docked at the planet
    let spaceship = world.spawn(SpaceShip { name: "Enterprise" }).id();
    world.entity_mut(spaceship).insert(BelongsToFaction(faction_a));
    world.entity_mut(spaceship).insert(DockedTo(planet));

    world.flush();

    // Build the complex query plan from plan.md:
    // SpaceShip($spaceship),
    // Faction($spaceship, $spaceship_faction),
    // DockedTo($spaceship, $planet),
    // Planet($planet),
    // RuledBy($planet, $planet_faction),
    // AlliedWith($spaceship_faction, $planet_faction)

    let mut builder = TypedQueryPlanBuilder::new(&mut world);

    let spaceship_term = builder.with::<SpaceShip>();          // 0
    let spaceship_faction_term = builder.with::<FactionTag>(); // 1
    let planet_term = builder.with::<Planet>();                // 2
    let planet_faction_term = builder.with::<FactionTag>();    // 3

    // Connect the terms via relationships
    builder.related_to::<BelongsToFaction>(spaceship_term, spaceship_faction_term);
    builder.related_to::<DockedTo>(spaceship_term, planet_term);
    builder.related_to::<RuledBy>(planet_term, planet_faction_term);
    builder.related_to::<AlliedWith>(spaceship_faction_term, planet_faction_term);

    let plan = builder.build(spaceship_term);

    // Execute the plan
    unsafe {
        let world_cell = world.as_unsafe_world_cell_readonly();
        let results = plan.execute(spaceship, world_cell);

        // This should find the spaceship because:
        // - It's docked at Earth
        // - It belongs to Faction A
        // - Earth is ruled by Faction B
        // - Faction A is allied with Faction B

        println!("Found {} matching combinations", results.len());
        for entities in &results {
            println!(
                "Spaceship: {:?}, Faction: {:?}, Planet: {:?}, Ruler: {:?}",
                entities[0], entities[1], entities[2], entities[3]
            );
        }
    }
}

// ============================================================================
// EXAMPLE 3: Adding Multiple Components to a Term
// ============================================================================

#[derive(Component)]
struct Name(&'static str);

#[derive(Component)]
struct Position { x: f32, y: f32 }

#[derive(Component)]
struct Velocity { dx: f32, dy: f32 }

fn example_multiple_components_per_term() {
    use crate::hierarchy::ChildOf;

    let mut world = World::new();

    // Build a query that accesses multiple components on each term
    let mut builder = TypedQueryPlanBuilder::new(&mut world);

    // Child term: Position + Velocity
    let child_term = builder.with::<Position>();
    builder.add_read::<Velocity>(child_term);

    // Parent term: Position + Name
    let parent_term = builder.with::<Position>();
    builder.add_read::<Name>(parent_term);

    // Connect via ChildOf
    builder.related_to::<ChildOf>(child_term, parent_term);

    let plan = builder.build(child_term);

    // Create test data
    let parent = world.spawn((
        Position { x: 100.0, y: 100.0 },
        Name("Parent"),
    )).id();

    let child = world.spawn((
        Position { x: 10.0, y: 10.0 },
        Velocity { dx: 1.0, dy: 0.5 },
        ChildOf(parent),
    )).id();

    world.flush();

    // Execute
    unsafe {
        let world_cell = world.as_unsafe_world_cell_readonly();
        let results = plan.execute(child, world_cell);

        assert_eq!(results.len(), 1);
        // Could access Position, Velocity on child (term 0)
        // Could access Position, Name on parent (term 1)
    }
}

// ============================================================================
// EXAMPLE 4: Filter Usage
// ============================================================================

#[derive(Component)]
struct Enemy;

#[derive(Component)]
struct Friendly;

fn example_with_filters() {
    use crate::hierarchy::ChildOf;

    let mut world = World::new();

    // Query for children that are enemies, with parents that are NOT enemies
    let mut builder = TypedQueryPlanBuilder::new(&mut world);

    let child_term = builder.with::<Enemy>();
    let parent_term = builder.term();
    builder.without::<Enemy>(parent_term);  // Parent should not be an enemy

    builder.related_to::<ChildOf>(child_term, parent_term);

    let plan = builder.build(child_term);

    // This would find enemy children of non-enemy parents
}

// ============================================================================
// Integration Notes
// ============================================================================

// To fully integrate this with Bevy's query system, you would:
//
// 1. Create a QueryState<Dynamic> with a pre-built plan:
//    let mut query_state = QueryState::from_builder(|builder| {
//        // Build plan...
//    });
//
// 2. Use it in a system:
//    fn my_system(query: Query<Dynamic>) {
//        for item in query.iter() {
//            for dynamic_match in item.iter() {
//                // Access matched entities
//            }
//        }
//    }
//
// 3. Or use QueryState directly:
//    let mut query_state = /* ... */;
//    for item in query_state.iter(&world) {
//        for dynamic_match in item.iter() {
//            let entity_ref = unsafe { dynamic_match.entity_ref(0) };
//            let component = entity_ref.get::<MyComponent>();
//        }
//    }

