use crate::{
    entity::Entity,
    identifier::masks::{IdentifierMask, HIGH_MASK},
    world::World,
};
use bevy_utils::EntityHashMap;

/// Operation to map all contained [`Entity`] fields in a type to new values.
///
/// As entity IDs are valid only for the [`World`] they're sourced from, using [`Entity`]
/// as references in components copied from another world will be invalid. This trait
/// allows defining custom mappings for these references via a [`Mapper`], which is a type that knows
/// how to perform entity mapping. (usually by using a [`EntityHashMap<Entity, Entity>`])
///
/// [`MapEntities`] is already implemented for [`Entity`], so you just need to call [`Entity::map_entities`]
/// for all the [`Entity`] fields in your type.
///
/// Implementing this trait correctly is required for properly loading components
/// with entity references from scenes.
///
/// ## Example
///
/// ```
/// use bevy_ecs::prelude::*;
/// use bevy_ecs::entity::{EntityMapper, MapEntities, Mapper, SimpleEntityMapper};
///
/// #[derive(Component)]
/// struct Spring {
///     a: Entity,
///     b: Entity,
/// }
///
/// impl MapEntities for Spring {
///     fn map_entities<M: Mapper>(&mut self, entity_mapper: &mut M) {
///         self.a.map_entities(entity_mapper);
///         self.b.map_entities(entity_mapper)
///     }
/// }
/// ```
///
pub trait MapEntities {
    /// Updates all [`Entity`] references stored inside using `entity_mapper`.
    fn map_entities<M: Mapper>(&mut self, entity_mapper: &mut M);
}

/// This traits defines a type that knows how to map [`Entity`] references.
///
/// Two implementations are provided:
/// - `SimpleEntityMapper`: tries to map the [`Entity`] reference, but if it can't, it returns the same [`Entity`] reference.
/// - `EntityMapper`: tries to map the [`Entity`] reference, but if it can't, it allocates a new [`Entity`] reference.
pub trait Mapper {
    /// Map an entity to another entity
    fn map(&mut self, entity: Entity) -> Entity;
}

/// Similar to `EntityMapper`, but does not allocate new [`Entity`] references in case we couldn't map the entity.
pub struct SimpleEntityMapper<'m> {
    map: &'m EntityHashMap<Entity, Entity>,
}

impl Mapper for SimpleEntityMapper<'_> {
    /// Map the entity to another entity, or return the same entity if we couldn't map it.
    fn map(&mut self, entity: Entity) -> Entity {
        self.get(entity).unwrap_or(entity)
    }
}

impl<'m> SimpleEntityMapper<'m> {
    /// Creates a new `SimpleEntityMapper` from an [`EntityHashMap<Entity, Entity>`].
    pub fn new(map: &'m EntityHashMap<Entity, Entity>) -> Self {
        Self { map }
    }

    /// Returns the corresponding mapped entity or None if it is absent.
    pub fn get(&self, entity: Entity) -> Option<Entity> {
        self.map.get(&entity).copied()
    }

    /// Gets a reference to the underlying [`EntityHashMap<Entity, Entity>`].
    pub fn get_map(&'m self) -> &'m EntityHashMap<Entity, Entity> {
        self.map
    }
}

impl Mapper for EntityMapper<'_> {
    /// Returns the corresponding mapped entity or reserves a new dead entity ID if it is absent.
    fn map(&mut self, entity: Entity) -> Entity {
        self.get_or_reserve(entity)
    }
}

/// A wrapper for [`EntityHashMap<Entity, Entity>`], augmenting it with the ability to allocate new [`Entity`] references in a destination
/// world. These newly allocated references are guaranteed to never point to any living entity in that world.
///
/// References are allocated by returning increasing generations starting from an internally initialized base
/// [`Entity`]. After it is finished being used by [`MapEntities`] implementations, this entity is despawned and the
/// requisite number of generations reserved.
pub struct EntityMapper<'m> {
    /// A mapping from one set of entities to another.
    ///
    /// This is typically used to coordinate data transfer between sets of entities, such as between a scene and the world
    /// or over the network. This is required as [`Entity`] identifiers are opaque; you cannot and do not want to reuse
    /// identifiers directly.
    ///
    /// On its own, a [`EntityHashMap<Entity, Entity>`] is not capable of allocating new entity identifiers, which is needed to map references
    /// to entities that lie outside the source entity set. This functionality can be accessed through [`EntityMapper::world_scope()`].
    map: &'m mut EntityHashMap<Entity, Entity>,
    /// A base [`Entity`] used to allocate new references.
    dead_start: Entity,
    /// The number of generations this mapper has allocated thus far.
    generations: u32,
}

impl<'m> EntityMapper<'m> {
    /// Returns the corresponding mapped entity or reserves a new dead entity ID if it is absent.
    pub fn get_or_reserve(&mut self, entity: Entity) -> Entity {
        if let Some(&mapped) = self.map.get(&entity) {
            return mapped;
        }

        // this new entity reference is specifically designed to never represent any living entity
        let new = Entity::from_raw_and_generation(
            self.dead_start.index(),
            IdentifierMask::inc_masked_high_by(self.dead_start.generation, self.generations),
        );

        // Prevent generations counter from being a greater value than HIGH_MASK.
        self.generations = (self.generations + 1) & HIGH_MASK;

        self.map.insert(entity, new);

        new
    }

    /// Gets a reference to the underlying [`EntityHashMap<Entity, Entity>`].
    pub fn get_map(&'m self) -> &'m EntityHashMap<Entity, Entity> {
        self.map
    }

    /// Gets a mutable reference to the underlying [`EntityHashMap<Entity, Entity>`].
    pub fn get_map_mut(&'m mut self) -> &'m mut EntityHashMap<Entity, Entity> {
        self.map
    }

    /// Creates a new [`EntityMapper`], spawning a temporary base [`Entity`] in the provided [`World`]
    fn new(map: &'m mut EntityHashMap<Entity, Entity>, world: &mut World) -> Self {
        Self {
            map,
            // SAFETY: Entities data is kept in a valid state via `EntityMapper::world_scope`
            dead_start: unsafe { world.entities_mut().alloc() },
            generations: 0,
        }
    }

    /// Reserves the allocated references to dead entities within the world. This frees the temporary base
    /// [`Entity`] while reserving extra generations via [`crate::entity::Entities::reserve_generations`]. Because this
    /// renders the [`EntityMapper`] unable to safely allocate any more references, this method takes ownership of
    /// `self` in order to render it unusable.
    fn finish(self, world: &mut World) {
        // SAFETY: Entities data is kept in a valid state via `EntityMap::world_scope`
        let entities = unsafe { world.entities_mut() };
        assert!(entities.free(self.dead_start).is_some());
        assert!(entities.reserve_generations(self.dead_start.index(), self.generations));
    }

    /// Creates an [`EntityMapper`] from a provided [`World`] and [`EntityHashMap<Entity, Entity>`], then calls the
    /// provided function with it. This allows one to allocate new entity references in this [`World`] that are
    /// guaranteed to never point at a living entity now or in the future. This functionality is useful for safely
    /// mapping entity identifiers that point at entities outside the source world. The passed function, `f`, is called
    /// within the scope of this world. Its return value is then returned from `world_scope` as the generic type
    /// parameter `R`.
    pub fn world_scope<R>(
        entity_map: &'m mut EntityHashMap<Entity, Entity>,
        world: &mut World,
        f: impl FnOnce(&mut World, &mut Self) -> R,
    ) -> R {
        let mut mapper = Self::new(entity_map, world);
        let result = f(world, &mut mapper);
        mapper.finish(world);
        result
    }
}

#[cfg(test)]
mod tests {
    use bevy_utils::EntityHashMap;

    use crate::{
        entity::map_entities::Mapper,
        entity::{Entity, EntityMapper, SimpleEntityMapper},
        world::World,
    };

    #[test]
    fn simple_entity_mapper() {
        const FIRST_IDX: u32 = 1;
        const SECOND_IDX: u32 = 2;

        const MISSING_IDX: u32 = 10;

        let mut map = EntityHashMap::default();
        map.insert(Entity::from_raw(FIRST_IDX), Entity::from_raw(SECOND_IDX));
        let mut mapper = SimpleEntityMapper::new(&map);

        // entity is mapped correctly if it exists in the map
        assert_eq!(
            mapper.map(Entity::from_raw(FIRST_IDX)),
            Entity::from_raw(SECOND_IDX)
        );

        // entity is just returned as is if it does not exist in the map
        assert_eq!(
            mapper.map(Entity::from_raw(MISSING_IDX)),
            Entity::from_raw(MISSING_IDX)
        );
    }

    #[test]
    fn entity_mapper() {
        const FIRST_IDX: u32 = 1;
        const SECOND_IDX: u32 = 2;

        let mut map = EntityHashMap::default();
        let mut world = World::new();
        let mut mapper = EntityMapper::new(&mut map, &mut world);

        let mapped_ent = Entity::from_raw(FIRST_IDX);
        let dead_ref = mapper.get_or_reserve(mapped_ent);

        assert_eq!(
            dead_ref,
            mapper.get_or_reserve(mapped_ent),
            "should persist the allocated mapping from the previous line"
        );
        assert_eq!(
            mapper.get_or_reserve(Entity::from_raw(SECOND_IDX)).index(),
            dead_ref.index(),
            "should re-use the same index for further dead refs"
        );

        mapper.finish(&mut world);
        // Next allocated entity should be a further generation on the same index
        let entity = world.spawn_empty().id();
        assert_eq!(entity.index(), dead_ref.index());
        assert!(entity.generation() > dead_ref.generation());
    }

    #[test]
    fn world_scope_reserves_generations() {
        let mut map = EntityHashMap::default();
        let mut world = World::new();

        let dead_ref = EntityMapper::world_scope(&mut map, &mut world, |_, mapper| {
            mapper.get_or_reserve(Entity::from_raw(0))
        });

        // Next allocated entity should be a further generation on the same index
        let entity = world.spawn_empty().id();
        assert_eq!(entity.index(), dead_ref.index());
        assert!(entity.generation() > dead_ref.generation());
    }
}
