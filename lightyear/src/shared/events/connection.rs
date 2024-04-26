/*! Defines a [`ConnectionEvents`] struct that is used to store all events that are received from a connection
 */
use std::iter;

use bevy::prelude::{Component, Entity, Resource};
use bevy::utils::HashMap;
use tracing::trace;

use crate::_reexport::{FromType, MessageProtocol};
#[cfg(feature = "leafwing")]
use crate::inputs::leafwing::{InputMessage, LeafwingUserAction};
use crate::packet::message::Message;
use crate::prelude::Tick;
use crate::protocol::channel::ChannelKind;
use crate::protocol::message::MessageKind;
use crate::protocol::{EventContext, Protocol};

// TODO: don't make fields pub but instead make accessors
#[derive(Debug, Resource)]
pub struct ConnectionEvents<P: Protocol> {
    // replication
    pub spawns: Vec<Entity>,
    pub despawns: Vec<Entity>,

    // TODO: [IMPORTANT]: add ticks as well?
    // - should we just return the latest update for a given component/entity, or all of them?
    // - should we have a way to get the updates/inserts/removes for a given entity?

    // TODO: key by entity or by kind?
    // TODO: include the actual value in the event, or just the type? let's just include the type for now
    pub component_inserts: HashMap<P::ComponentKinds, Vec<Entity>>,
    // pub insert_components: HashMap<Entity, Vec<P::Components>>,
    pub component_removes: HashMap<P::ComponentKinds, Vec<Entity>>,
    // TODO: here as well, we could only include the type.. we already apply the changes to the entity directly, so users could keep track of changes
    //  let's just start with the kind...
    //  also, normally the updates are sequenced
    pub component_updates: HashMap<P::ComponentKinds, Vec<Entity>>,
    // // TODO: what happens if we receive on the same frame an Update for tick 4 and update for tick 10?
    // //  can we just discard the older one? what about for inserts/removes?
    // pub component_updates: EntityHashMap<Entity, HashMap<P::ComponentKinds, Tick>>,
    // components_with_updates: HashSet<P::ComponentKinds>,

    // How can i easily get the events (inserts/adds/removes) for a given entity? add components on that entity
    // that track that?
    empty: bool,
}

impl<P: Protocol> Default for ConnectionEvents<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Protocol> ConnectionEvents<P> {
    pub fn new() -> Self {
        Self {
            // replication
            spawns: Vec::new(),
            despawns: Vec::new(),
            component_inserts: Default::default(),
            component_removes: Default::default(),
            component_updates: Default::default(),
            // components_with_updates: Default::default(),
            // bookkeeping
            empty: true,
        }
    }

    pub fn clear(&mut self) {
        self.spawns.clear();
        self.despawns.clear();
        self.component_inserts.clear();
        self.component_removes.clear();
        self.component_updates.clear();
        self.empty = true;
    }

    pub fn is_empty(&self) -> bool {
        self.empty
    }

    pub(crate) fn push_spawn(&mut self, entity: Entity) {
        trace!(?entity, "Received entity spawn");
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("entity_spawn").increment(1);
        }
        self.spawns.push(entity);
        self.empty = false;
    }

    pub(crate) fn push_despawn(&mut self, entity: Entity) {
        trace!(?entity, "Received entity despawn");
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("entity_despawn").increment(1);
        }
        self.despawns.push(entity);
        self.empty = false;
    }

    pub(crate) fn push_insert_component(
        &mut self,
        entity: Entity,
        component: P::ComponentKinds,
        tick: Tick,
    ) {
        trace!(?entity, ?component, "Received insert component");
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("component_insert", "kind" => component.to_string()).increment(1);
        }
        self.component_inserts
            .entry(component)
            .or_default()
            .push(entity);
        // .push((entity, tick));
        self.empty = false;
    }

    pub(crate) fn push_remove_component(
        &mut self,
        entity: Entity,
        component: P::ComponentKinds,
        tick: Tick,
    ) {
        trace!(?entity, ?component, "Received remove component");
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("component_remove", "kind" => component.to_string()).increment(1);
        }
        self.component_removes
            .entry(component)
            .or_default()
            .push(entity);
        // .push((entity, tick));
        self.empty = false;
    }

    // TODO: how do distinguish between multiple updates for the same component/entity? add ticks?
    pub(crate) fn push_update_component(
        &mut self,
        entity: Entity,
        component: P::ComponentKinds,
        tick: Tick,
    ) {
        trace!(?entity, ?component, "Received update component");
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("component_update", "kind" => component.to_string()).increment(1);
        }
        // self.components_with_updates.insert(component.clone());
        // self.component_updates
        //     .entry(entity)
        //     .or_default()
        //     .entry(component)
        //     .and_modify(|t| {
        //         if tick > *t {
        //             *t = tick;
        //         }
        //     })
        //     .or_insert(tick);

        self.component_updates
            .entry(component)
            .or_default()
            .push(entity);
        // .push((entity, tick));
        self.empty = false;
    }
}

pub trait IterEntitySpawnEvent<Ctx: EventContext = ()> {
    fn into_iter_entity_spawn(&mut self) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>;
    fn has_entity_spawn(&self) -> bool;
}

impl<P: Protocol> IterEntitySpawnEvent for ConnectionEvents<P> {
    fn into_iter_entity_spawn(&mut self) -> Box<dyn Iterator<Item = (Entity, ())> + '_> {
        let spawns = std::mem::take(&mut self.spawns);
        Box::new(spawns.into_iter().map(|entity| (entity, ())))
    }

    fn has_entity_spawn(&self) -> bool {
        !self.spawns.is_empty()
    }
}

pub trait IterEntityDespawnEvent<Ctx: EventContext = ()> {
    fn into_iter_entity_despawn(&mut self) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>;
    fn has_entity_despawn(&self) -> bool;
}

impl<P: Protocol> IterEntityDespawnEvent for ConnectionEvents<P> {
    fn into_iter_entity_despawn(&mut self) -> Box<dyn Iterator<Item = (Entity, ())> + '_> {
        let despawns = std::mem::take(&mut self.despawns);
        Box::new(despawns.into_iter().map(|entity| (entity, ())))
    }

    fn has_entity_despawn(&self) -> bool {
        !self.despawns.is_empty()
    }
}

/// Iterate through all the events for a given entity
pub trait IterComponentUpdateEvent<P: Protocol, Ctx: EventContext = ()> {
    /// Find all the updates of component C
    fn iter_component_update<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>
    where
        P::ComponentKinds: FromType<C>;

    /// Is there any update for component C
    fn has_component_update<C: Component>(&self) -> bool
    where
        P::ComponentKinds: FromType<C>;

    // /// Find all the updates of component C for a given entity
    // fn get_component_update<C: Component>(&self, entity: Entity) -> Option<Ctx>
    // where
    //     P::ComponentKinds: FromType<C>;
}

impl<P: Protocol> IterComponentUpdateEvent<P> for ConnectionEvents<P> {
    fn iter_component_update<C: Component>(&mut self) -> Box<dyn Iterator<Item = (Entity, ())> + '_>
    where
        P::ComponentKinds: FromType<C>,
    {
        let component_kind = <P::ComponentKinds as FromType<C>>::from_type();
        if let Some(data) = self.component_updates.remove(&component_kind) {
            return Box::new(data.into_iter().map(|entity| (entity, ())));
        }
        Box::new(iter::empty())
        // Box::new(
        //     self.component_updates
        //         .iter()
        //         .filter_map(|(entity, updates)| {
        //             updates.get(&C::into_kind()).map(|tick| (*entity, *tick))
        //         }),
        // )
    }

    fn has_component_update<C: Component>(&self) -> bool
    where
        P::ComponentKinds: FromType<C>,
    {
        let component_kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.component_updates.contains_key(&component_kind)
        // self.components_with_updates.contains(&C::into_kind())
    }

    // // TODO: is it possible to receive multiple updates for the same component/entity?
    // //  it shouldn't be possible for a Sequenced channel,
    // //  maybe just take the first value that matches, then?
    // fn get_component_update<C: Component>(&self, entity: Entity) -> Option<()>
    // where
    //     P::ComponentKinds: FromType<C>,
    // {
    //     todo!()
    //     // self.component_updates
    //     //     .get(&entity)
    //     //     .map(|updates| updates.get(&C::into_kind()).cloned())
    //     //     .flatten()
    // }
}

pub trait IterComponentRemoveEvent<P: Protocol, Ctx: EventContext = ()> {
    fn iter_component_remove<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>
    where
        P::ComponentKinds: FromType<C>;
    fn has_component_remove<C: Component>(&self) -> bool
    where
        P::ComponentKinds: FromType<C>;
}

// TODO: move these implementations to client?
impl<P: Protocol> IterComponentRemoveEvent<P> for ConnectionEvents<P> {
    fn iter_component_remove<C: Component>(&mut self) -> Box<dyn Iterator<Item = (Entity, ())> + '_>
    where
        P::ComponentKinds: FromType<C>,
    {
        let component_kind = <P::ComponentKinds as FromType<C>>::from_type();
        if let Some(data) = self.component_removes.remove(&component_kind) {
            return Box::new(data.into_iter().map(|entity| (entity, ())));
        }
        Box::new(iter::empty())
    }

    fn has_component_remove<C: Component>(&self) -> bool
    where
        P::ComponentKinds: FromType<C>,
    {
        let component_kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.component_removes.contains_key(&component_kind)
    }
}

pub trait IterComponentInsertEvent<P: Protocol, Ctx: EventContext = ()> {
    fn iter_component_insert<C: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, Ctx)> + '_>
    where
        P::ComponentKinds: FromType<C>;
    fn has_component_insert<C: Component>(&self) -> bool
    where
        P::ComponentKinds: FromType<C>;
}

impl<P: Protocol> IterComponentInsertEvent<P> for ConnectionEvents<P> {
    fn iter_component_insert<C: Component>(&mut self) -> Box<dyn Iterator<Item = (Entity, ())> + '_>
    where
        P::ComponentKinds: FromType<C>,
    {
        let component_kind = <P::ComponentKinds as FromType<C>>::from_type();
        if let Some(data) = self.component_inserts.remove(&component_kind) {
            return Box::new(data.into_iter().map(|entity| (entity, ())));
        }
        Box::new(iter::empty())
    }

    fn has_component_insert<C: Component>(&self) -> bool
    where
        P::ComponentKinds: FromType<C>,
    {
        let component_kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.component_inserts.contains_key(&component_kind)
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::protocol::*;

    use super::*;

    // #[test]
    // fn test_iter_messages() {
    //     let mut events = ConnectionEvents::<MyProtocol>::new();
    //     let channel_kind_1 = ChannelKind::of::<Channel1>();
    //     let channel_kind_2 = ChannelKind::of::<Channel2>();
    //     let message1_a = Message1("hello".to_string());
    //     let message1_b = Message1("world".to_string());
    //     events.push_message(
    //         channel_kind_1,
    //         MyMessageProtocol::Message1(message1_a.clone()),
    //     );
    //     events.push_message(
    //         channel_kind_2,
    //         MyMessageProtocol::Message1(message1_b.clone()),
    //     );
    //     events.push_message(channel_kind_1, MyMessageProtocol::Message2(Message2(1)));
    //
    //     // check that we have the correct messages
    //     let messages: Vec<Message1> = events.into_iter_messages().map(|(m, _)| m).collect();
    //     assert!(messages.contains(&message1_a));
    //     assert!(messages.contains(&message1_b));
    //
    //     // check that there are no more message of that kind in the events
    //     assert!(!events.messages.contains_key(&MessageKind::of::<Message1>()));
    //
    //     // check that we still have the other message kinds
    //     assert!(events.messages.contains_key(&MessageKind::of::<Message2>()));
    // }

    // #[test]
    // fn test_iter_component_updates() {
    //     let mut events = ConnectionEvents::<MyProtocol>::new();
    //     let channel_kind_1 = ChannelKind::of::<Channel1>();
    //     let channel_kind_2 = ChannelKind::of::<Channel2>();
    //     let entity_1 = Entity::from_raw(1);
    //     let entity_2 = Entity::from_raw(2);
    //     events.push_update_component(entity_1, MyComponentsProtocolKind::Component1, Tick(1));
    //     events.push_update_component(entity_1, MyComponentsProtocolKind::Component2, Tick(2));
    //     events.push_update_component(entity_2, MyComponentsProtocolKind::Component2, Tick(3));
    //
    //     assert!(events
    //         .get_component_update::<Component1>(entity_2)
    //         .is_none());
    //     assert_eq!(
    //         events.get_component_update::<Component2>(entity_2),
    //         Some(Tick(3))
    //     );
    //
    //     let component_1_updates: HashSet<(Entity, Tick)> =
    //         events.iter_component_update::<Component1>().collect();
    //     assert!(component_1_updates.contains(&(entity_1, Tick(1))));
    //
    //     let component_2_updates: HashSet<(Entity, Tick)> =
    //         events.iter_component_update::<Component2>().collect();
    //     assert!(component_2_updates.contains(&(entity_1, Tick(2))));
    //     assert!(component_2_updates.contains(&(entity_2, Tick(3))));
    //
    //     let component_3_updates: HashSet<(Entity, Tick)> =
    //         events.iter_component_update::<Component3>().collect();
    //     assert!(component_3_updates.is_empty());
    // }
}
