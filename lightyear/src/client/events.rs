//! Wrapper around [`ConnectionEvents`] that adds client-specific functionality
//!
use crate::connection::events::ConnectionEvents;
use crate::prelude::Tick;
use crate::protocol::Protocol;

pub struct ClientEvents<P: Protocol> {
    // cannot include connection/disconnection directly into ConnectionEvents, because we remove
    // the connection event upon disconnection
    connection: bool,
    disconnection: bool,
    events: ConnectionEvents<P>,
}

pub type ConnectEvent = crate::shared::events::ConnectEvent<()>;
pub type DisconnectEvent = crate::shared::events::DisconnectEvent<()>;
pub type InputEvent<I> = crate::shared::events::InputEvent<I, ()>;

pub type EntitySpawnEvent = crate::shared::events::EntitySpawnEvent<()>;
pub type EntityDespawnEvent = crate::shared::events::EntityDespawnEvent<()>;
pub type ComponentUpdateEvent<C> = crate::shared::events::ComponentUpdateEvent<C, ()>;
pub type ComponentInsertEvent<C> = crate::shared::events::ComponentInsertEvent<C, ()>;
pub type ComponentRemoveEvent<C> = crate::shared::events::ComponentRemoveEvent<C, ()>;
pub type MessageEvent<M> = crate::shared::events::MessageEvent<M, ()>;
