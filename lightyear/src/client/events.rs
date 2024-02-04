//! Bevy [`Event`](bevy::prelude::Event) that are emitted when certain network events occur on the client
//!
//! You can use this to react to network events in your game systems.
//! ```rust,no_run
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

/// Bevy [`Event`](bevy::prelude::Event) emitted on the client on the frame where the connection is established
pub type ConnectEvent = crate::shared::events::ConnectEvent<()>;
/// Bevy [`Event`](bevy::prelude::Event) emitted on the client on the frame where the connection is disconnected
pub type DisconnectEvent = crate::shared::events::DisconnectEvent<()>;
pub type InputEvent<I> = crate::shared::events::InputEvent<I, ()>;
/// Bevy [`Event`](bevy::prelude::Event) emitted on the client when a EntitySpawn replication message is received
pub type EntitySpawnEvent = crate::shared::events::EntitySpawnEvent<()>;
/// Bevy [`Event`](bevy::prelude::Event) emitted on the client when a EntityDespawn replication message is received
pub type EntityDespawnEvent = crate::shared::events::EntityDespawnEvent<()>;
/// Bevy [`Event`](bevy::prelude::Event) emitted on the client when a ComponentUpdate replication message is received
pub type ComponentUpdateEvent<C> = crate::shared::events::ComponentUpdateEvent<C, ()>;
/// Bevy [`Event`](bevy::prelude::Event) emitted on the client when a ComponentInsert replication message is received
pub type ComponentInsertEvent<C> = crate::shared::events::ComponentInsertEvent<C, ()>;
/// Bevy [`Event`](bevy::prelude::Event) emitted on the client when a ComponentRemove replication message is received
pub type ComponentRemoveEvent<C> = crate::shared::events::ComponentRemoveEvent<C, ()>;
/// Bevy [`Event`](bevy::prelude::Event) emitted on the client when a (non-replication) message is received
pub type MessageEvent<M> = crate::shared::events::MessageEvent<M, ()>;
