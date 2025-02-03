//! Bevy [`Event`] that are emitted when certain network events occur on the client
//!
//! You can use this to react to network events in your game systems.
//! ```rust,ignore
//! use bevy::ecs::event::EventId;
//! fn handle_message(mut messages: EventReader<MessageEvent<MyMessage>>) {
//!   for event in messages.read() {
//!     // the event has two functions `message()` and `context()`
//!     // `context()` is currently unused but is reserved for future uses (e.g. to get the sender of the message, or the tick it was sent on)
//!     let message = event.message();
//!     // do something with the message
//!   }
//! }
//! ```

use crate::client::connection::ConnectionManager;
use crate::connection::client::ConnectionError;
use crate::prelude::ClientId;
use crate::shared::events::plugin::EventsPlugin;
use crate::shared::events::systems::push_component_events;
use crate::shared::sets::{ClientMarker, InternalMainSet};
use bevy::app::{App, Plugin, PreUpdate};
use bevy::prelude::{Component, Event, IntoSystemConfigs};

/// Plugin that handles generating bevy [`Events`](Event) related to networking and replication
#[derive(Default)]
pub struct ClientEventsPlugin;

impl Plugin for ClientEventsPlugin {
    fn build(&self, app: &mut App) {
        app
            // EVENTS
            .add_event::<ConnectEvent>()
            .add_event::<DisconnectEvent>()
            // PLUGIN
            .add_plugins(EventsPlugin::<ConnectionManager>::default());
    }
}

pub(crate) fn emit_replication_events<C: Component>(app: &mut App) {
    app.add_event::<ComponentUpdateEvent<C>>();
    app.add_event::<ComponentInsertEvent<C>>();
    app.add_event::<ComponentRemoveEvent<C>>();
    app.add_systems(
        PreUpdate,
        push_component_events::<C, ConnectionManager>
            .in_set(InternalMainSet::<ClientMarker>::EmitEvents),
    );
}

/// Bevy [`Event`] emitted on the client on the frame where the connection is established
///
/// We keep this separate from the server's ConnectEvent so that we have different events emitted on the client
/// and the server when running in HostServer mode
#[derive(Event)]
pub struct ConnectEvent(ClientId);

impl ConnectEvent {
    pub fn new(client_id: ClientId) -> Self {
        Self(client_id)
    }
    pub fn client_id(&self) -> ClientId {
        self.0
    }
}

/// Bevy [`Event`] emitted on the client on the frame where the connection is disconnected
#[derive(Event, Default, Debug)]
pub struct DisconnectEvent {
    pub reason: Option<ConnectionError>,
}

/// Bevy [`Event`] emitted on the client to indicate the user input for the tick
pub type InputEvent<I> = crate::shared::events::components::InputEvent<I, ()>;
/// Bevy [`Event`] emitted on the client when a EntitySpawn replication message is received
pub type EntitySpawnEvent = crate::shared::events::components::EntitySpawnEvent<()>;
/// Bevy [`Event`] emitted on the client when a EntityDespawn replication message is received
pub type EntityDespawnEvent = crate::shared::events::components::EntityDespawnEvent<()>;
/// Bevy [`Event`] emitted on the client when a ComponentUpdate replication message is received
pub type ComponentUpdateEvent<C> = crate::shared::events::components::ComponentUpdateEvent<C, ()>;
/// Bevy [`Event`] emitted on the client when a ComponentInsert replication message is received
pub type ComponentInsertEvent<C> = crate::shared::events::components::ComponentInsertEvent<C, ()>;
/// Bevy [`Event`] emitted on the client when a ComponentRemove replication message is received
pub type ComponentRemoveEvent<C> = crate::shared::events::components::ComponentRemoveEvent<C, ()>;
/// Bevy [`Event`] emitted on the client when a (non-replication) message is received
pub type MessageEvent<M> = crate::shared::events::message::MessageEvent<M>;

#[cfg(test)]
mod tests {
    use crate::client::connection::ConnectionManager;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, IntegerEvent};
    use bevy::prelude::{EventReader, ResMut, Resource, Trigger, Update};

    #[derive(Resource, Default)]
    struct Counter(usize);

    fn count_events(mut counter: ResMut<Counter>, mut events: EventReader<IntegerEvent>) {
        for event in events.read() {
            assert_eq!(event.0, 2);
            counter.0 += 1;
        }
    }

    fn observe_events(trigger: Trigger<IntegerEvent>, mut counter: ResMut<Counter>) {
        assert_eq!(trigger.event().0, 2);
        counter.0 += 1;
    }

    /// Check that client sending an event works correctly:
    /// - the event gets buffered to EventWriter on the server
    /// - it works for the Local client in HostServer mode (the server still receives the event)
    // TODO: - the server can re-broadcast the event to another client
    #[test]
    fn test_client_send_event_buffered() {
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_events);

        // client send event to server
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ConnectionManager>()
            .send_event::<Channel1, _>(&IntegerEvent(2))
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);

        // local client send event to server
        stepper
            .server_app
            .world_mut()
            .resource_mut::<ConnectionManager>()
            .send_event::<Channel1, _>(&IntegerEvent(2))
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 2);
    }

    /// Check that client sending an event works correctly:
    /// - the event gets triggered
    /// - it works for the Local client in HostServer mode (the server still receives the event)
    // TODO: - the server can re-broadcast the event to another client
    #[test]
    fn test_client_send_event_triggered() {
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.server_app.add_observer(observe_events);

        // client send event to server
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ConnectionManager>()
            .trigger_event::<Channel1, _>(&IntegerEvent(2))
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);

        // local client send event to server
        stepper
            .server_app
            .world_mut()
            .resource_mut::<ConnectionManager>()
            .trigger_event::<Channel1, _>(&IntegerEvent(2))
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 2);
    }
}
