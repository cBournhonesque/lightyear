use std::collections::HashMap;

use bevy_ecs::event::Event;

use lightyear_shared::{ChannelKind, Protocol};

#[derive(Event)]
pub struct ConnectEvent;

#[derive(Event)]
pub struct DisconnectEvent;

#[derive(Event)]
pub struct MessageEvents<P: Protocol> {
    inner: HashMap<ChannelKind, P::Message>,
}
