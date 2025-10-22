# Dynamic Multi-Source Query Implementation

This document describes the implementation of dynamic multi-source queries that can follow relationships between entities.

## Architecture

The implementation consists of several interconnected components:

### 1. Query Plan (`plan.rs`)

The foundation of the system. A `QueryPlan` describes:
- **Terms**: Individual entity sources with their component access requirements
- **Relationships**: How terms are connected via relationship components
- **Execution**: A graph traversal algorithm that finds all matching entity combinations

#### Core Types:
- `QueryTerm`: Represents a single source with its `FilteredAccess`
- `QueryRelationship`: Describes a connection between two terms
- `QueryPlan`: The complete graph of terms and relationships
- `QueryPlanBuilder`: Low-level API for building plans
- `TypedQueryPlanBuilder`: High-level typed API for building plans

### 2. Dynamic Query (`dynamic.rs`)

A WorldQuery implementation that uses a QueryPlan to fetch multiple related entities.

#### Core Types:
- `Dynamic`: The WorldQuery type
- `DynamicState`: Contains the QueryPlan
- `DynamicFetch`: Fetch state for iteration
- `DynamicItem`: The result for one main entity (contains all matches)
- `DynamicMatch`: A single matched combination of entities

### 3. Relationship Accessor (`plan.rs`)

Type-erased way to traverse relationships:
- For `Relationship` components: Uses field offset to read the target entity
- For `RelationshipTarget` components: Uses iterator function to get all source entities

## Usage Examples

### Simple Parent-Child Query

```rust
use bevy_ecs::prelude::*;
use bevy_ecs::query::TypedQueryPlanBuilder;
use bevy_ecs::hierarchy::ChildOf;

#[derive(Component)]
struct Health(u32);

fn query_parent_child_health(world: &mut World) {
    // Build a plan: entities with Health -> their parents (also with Health)
    let mut builder = TypedQueryPlanBuilder::new(world);
    let child_term = builder.with::<Health>();
    let parent_term = builder.with::<Health>();
    builder.related_to::<ChildOf>(child_term, parent_term);

    let plan = builder.build(child_term);

    // Execute the plan for a specific child
    let child_entity = /* ... */;
    unsafe {
        let world_cell = world.as_unsafe_world_cell_readonly();
        let results = plan.execute(child_entity, world_cell);

        for match_result in results {
            let child = match_result[0];
            let parent = match_result[1];
            // Access components from both entities
        }
    }
}
```

### Complex Multi-Relationship Query

From the plan.md example:
```
SpaceShip($spaceship),
Faction($spaceship, $spaceship_faction),
DockedTo($spaceship, $planet),
Planet($planet),
RuledBy($planet, $planet_faction),
AlliedWith($spaceship_faction, $planet_faction)
```

Implementation:
```rust
let mut builder = TypedQueryPlanBuilder::new(&mut world);

let spaceship = builder.with::<SpaceShip>();      // 0
let spaceship_faction = builder.term();            // 1
let planet = builder.with::<Planet>();             // 2
let planet_faction = builder.term();               // 3

builder.related_to::<Faction>(spaceship, spaceship_faction);
builder.related_to::<DockedTo>(spaceship, planet);
builder.related_to::<RuledBy>(planet, planet_faction);
builder.related_to::<AlliedWith>(spaceship_faction, planet_faction);

let plan = builder.build(spaceship);
```

This finds all spaceships where:
- The spaceship is docked at a planet
- The spaceship belongs to a faction
- The planet is ruled by a faction
- The spaceship's faction is allied with the planet's faction

## Integration Points

### Current State
- ✅ `QueryPlan` - Core execution engine
- ✅ `QueryTerm` - Term matching
- ✅ `QueryRelationship` - Relationship traversal
- ✅ `Dynamic` - WorldQuery implementation
- ✅ `TypedQueryPlanBuilder` - Ergonomic builder API

### TODO for Full Integration
- [ ] Integrate with `QueryState` for caching and iteration
- [ ] Add archetype-level caching for the main term
- [ ] Support `FilteredEntityMut` for mutable access to related entities
- [ ] Add filter support (With/Without) on individual terms
- [ ] Optimize relationship traversal with caching
- [ ] Add support for transmuting Dynamic to typed queries for simple cases
- [ ] Integration with the query builder API
- [ ] Performance optimizations (graph compilation, join ordering, etc.)

## Design Decisions

### Why FilteredAccess?
Each term has its own `FilteredAccess` which:
- Defines which components can be read/written
- Includes filters (With/Without)
- Can be checked for conflicts with other queries

### Why Graph Traversal?
The recursive backtracking approach:
- Handles arbitrary relationship graphs (not just trees)
- Finds all matching combinations
- Can be optimized later with better algorithms

### Why Dynamic?
- Complex multi-source queries are hard to express in Rust's type system
- Dynamic allows runtime-constructed query plans
- Can be optimized by the query system based on runtime statistics
- Future: Could support transmuting to typed queries for simple tree patterns

## Performance Considerations

Current implementation is a prototype focused on correctness:
- No caching of intermediate results
- Recursive backtracking (could use iterative approach)
- No join reordering optimization
- Matches are computed per-entity (could batch)

Future optimizations:
- Cache relationship traversals
- Use archetype-level filtering for the main term
- Implement query planning (join reordering)
- Add indices for common relationship patterns
- Consider a compiled bytecode approach like Flecs

## Comparison with Flecs

Flecs uses a sophisticated query engine with:
- Query planning and optimization
- Variable binding and unification
- Compile-time query optimization

Our approach is simpler initially:
- Direct graph traversal
- No query optimization (yet)
- Clear separation between plan construction and execution

This makes it easier to understand and extend, with optimization opportunities preserved for later.

