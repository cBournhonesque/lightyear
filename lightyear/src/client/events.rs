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

use bevy::app::{App, Plugin, PreUpdate};
use bevy::prelude::{Component, Event, IntoSystemConfigs};

use crate::client::connection::ConnectionManager;
use crate::connection::client::DisconnectReason;
use crate::prelude::ClientId;
use crate::shared::events::plugin::EventsPlugin;
use crate::shared::events::systems::push_component_events;
use crate::shared::sets::{ClientMarker, InternalMainSet};

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
#[derive(Event, Default)]
pub struct DisconnectEvent {
    pub reason: Option<DisconnectReason>,
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
pub type MessageEvent<M> = crate::shared::events::components::MessageEvent<M, ()>;
