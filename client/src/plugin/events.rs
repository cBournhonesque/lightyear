use std::collections::HashMap;

use bevy::prelude::Event;

use lightyear_shared::{ChannelKind, Protocol};

#[derive(Event)]
pub struct MessageEvents<P: Protocol> {
    inner: HashMap<ChannelKind, P::Message>,
}
