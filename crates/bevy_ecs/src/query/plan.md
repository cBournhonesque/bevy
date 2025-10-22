I'm wondering how we would evolve the bevy Query type to support multi-term queries.

@quartermeister 's PR would already bring us a long way, since it would allow for things like
Query<(Entity, DockedTo<()>), With> (flecs: SpaceShip, (DockedTo, *))
Query<(Entity, DockedTo<(), With>), With> (flecs: SpaceShip($this), DockedTo($this, $Location), Planet($Location))

But not things like this are probably not possible
SpaceShip($spaceship),
Faction($spaceship, $spaceship_faction),
DockedTo($spaceship, $planet),
Planet($planet),
RuledBy($planet, $planet_faction),
AlliedWith($spaceship_faction, $planet_faction)

The thing is that bevy's syntax would be complicated by the fact that we need to specify at the same time what data we query (to set the accesses correctly for multithreading systems, etc.) and which entities we query

I guess these kind of complicated multi-term queries would only be dynamic, in which case we could do:
builder
.with::<Spaceship>(0)
.related_to::<Faction>(0, 1)
.with::<C>(1) // HERE: we add the extra components we want to access on any term
.related_to::<DockedTo>(0, 2)
.with::<Planet>(2)
.related_to::<RuledBy>(2, 3)
.related_to::<AlliedWith>(1, 3)

And then we would do
builder
.build::() // gives us a QueryState<Dynamic, ()>

Where Dynamic is roughly a wrapper around a tuple of FilteredEntityRef/Mut. Maybe we could then call things like dynamic.query<(&C1, &C2)>(1) which uses transmuting to return a QueryState for the term 1 of the query ($spaceship_faction). The builder needs to store the access for each term to be able to check if the transmutes are valid.

guess in this scenario my question is how things would related to @quartermeister 's bevyengine#21557.
IterQueryData and SingleEntityQueryData are definitely necessary pieces, no matter how multi-term queries are implemented
we still need init_nested_access, which would be used by Dynamic to provide the access of non-source terms.
would something like Query<(Entity, DockedTo<()>), With> use a nested QueryState, or would we want to convert that QueryState to the dynamic query plan, resolve it there, and then transmute it back to the original type?
would we be able to transmute something like
builder
.with(0)
.related_to(0, 1)
.with(1)

back to a Query<Faction<&C>> or is that too hard to do? (i.e. if the relationships in the query builder is a tree, it should be able to be transmuted back to a nested QueryStates form)
If it's not possible, then the Query<Faction<&C>> would be a nice UX gain for simple relationship queries. More complicated use-cases could fallback to using the dynamic query builder

So i guess if we were to attempt a prototype:
A) extend QueryBuilder to accept multiple query terms linked by relationships, similar to what james did: bevyengine@b56e1c7#diff-b40f95bb6879b8c95b1be48e381a6cb274db01b5919024eb359a4a2feaefb160R6
we can just do a Vec for now
B) hard: update the matching logic to not use only the access, but use the actual plan
i'm more blurry on this. It seems flecs has some crazy state machine inspired by prolog but I'd like to start with the simplest thing possible.
even without thinking about the logic, this requires a bunch of changes to our matching logic. The matching logic currently just checks if the current access matches. Instead we would be providing a more complex plan. If the plan has no multi-term it can just be the ComponentAccess (for API compatibility with existing QueryData)
C) introduce Dynamic, a WorldQuery that would let you access data dynamically for any of the terms of the query plan
D) support transmutes from parts of dynamic so that we could do dynamic.query::<(&C1, &C2, FilteredEntityRef)>(1) or maybe even dynamic.transmute::<(&C1, ChildOf<&C2, With>)>. @quartermeister (again!) paved the way with the amazing: bevyengine#18236


Note that we have access to this API for dynamic relationship access:
```rust
/// This enum describes a way to access the entities of [`Relationship`] and [`RelationshipTarget`] components
/// in a type-erased context.
#[derive(Debug, Clone, Copy)]
pub enum RelationshipAccessor {
    /// This component is a [`Relationship`].
    Relationship {
        /// Offset of the field containing [`Entity`] from the base of the component.
        ///
        /// Dynamic equivalent of [`Relationship::get`].
        entity_field_offset: usize,
        /// Value of [`RelationshipTarget::LINKED_SPAWN`] for the [`Relationship::RelationshipTarget`] of this [`Relationship`].
        linked_spawn: bool,
    },
    /// This component is a [`RelationshipTarget`].
    RelationshipTarget {
        /// Function that returns an iterator over all [`Entity`]s of this [`RelationshipTarget`]'s collection.
        ///
        /// Dynamic equivalent of [`RelationshipTarget::iter`].
        /// # Safety
        /// Passed pointer must point to the value of the same component as the one that this accessor was registered to.
        iter: for<'a> unsafe fn(Ptr<'a>) -> Box<dyn Iterator<Item = Entity> + 'a>,
        /// Value of [`RelationshipTarget::LINKED_SPAWN`] of this [`RelationshipTarget`].
        linked_spawn: bool,
    },
}

/// A type-safe convenience wrapper over [`RelationshipAccessor`].
pub struct ComponentRelationshipAccessor<C: ?Sized> {
    pub(crate) accessor: RelationshipAccessor,
    phantom: PhantomData<C>,
}

impl<C> ComponentRelationshipAccessor<C> {
    /// Create a new [`ComponentRelationshipAccessor`] for a [`Relationship`] component.
    /// # Safety
    /// `entity_field_offset` should be the offset from the base of this component and point to a field that stores value of type [`Entity`].
    /// This value can be obtained using the [`core::mem::offset_of`] macro.
    pub unsafe fn relationship(entity_field_offset: usize) -> Self
    where
        C: Relationship,
    {
        Self {
            accessor: RelationshipAccessor::Relationship {
                entity_field_offset,
                linked_spawn: C::RelationshipTarget::LINKED_SPAWN,
            },
            phantom: Default::default(),
        }
    }

    /// Create a new [`ComponentRelationshipAccessor`] for a [`RelationshipTarget`] component.
    pub fn relationship_target() -> Self
    where
        C: RelationshipTarget,
    {
        Self {
            accessor: RelationshipAccessor::RelationshipTarget {
                // Safety: caller ensures that `ptr` is of type `C`.
                iter: |ptr| unsafe { Box::new(RelationshipTarget::iter(ptr.deref::<C>())) },
                linked_spawn: C::LINKED_SPAWN,
            },
            phantom: Default::default(),
        }
    }
}

#[test]
  fn dynamically_traverse_hierarchy() {
      let mut world = World::new();
      let child_of_id = world.register_component::<ChildOf>();
      let children_id = world.register_component::<Children>();

      let parent = world.spawn_empty().id();
      let child = world.spawn_empty().id();
      world.entity_mut(child).insert(ChildOf(parent));
      world.flush();

      let children_ptr = world.get_by_id(parent, children_id).unwrap();
      let RelationshipAccessor::RelationshipTarget { iter, .. } = world
          .components()
          .get_info(children_id)
          .unwrap()
          .relationship_accessor()
          .unwrap()
      else {
          unreachable!()
      };
      // Safety: `children_ptr` contains value of the same type as the one this accessor was registered for.
      let children: Vec<_> = unsafe { iter(children_ptr).collect() };
      assert_eq!(children, alloc::vec![child]);

      let child_of_ptr = world.get_by_id(child, child_of_id).unwrap();
      let RelationshipAccessor::Relationship {
          entity_field_offset,
          ..
      } = world
          .components()
          .get_info(child_of_id)
          .unwrap()
          .relationship_accessor()
          .unwrap()
      else {
          unreachable!()
      };
      // Safety:
      // - offset is in bounds, aligned and has the same lifetime as the original pointer.
      // - value at offset is guaranteed to be a valid Entity
      let child_of_entity: Entity =
          unsafe { *child_of_ptr.byte_add(*entity_field_offset).deref() };
        assert_eq!(child_of_entity, parent);
    }
```
