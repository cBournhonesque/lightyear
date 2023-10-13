use std::collections::HashMap;

use crate::{BitSerializable, ChannelContainer, ChannelKind};

pub trait ChannelProtocol {
    fn channels<P: BitSerializable>(&self) -> HashMap<ChannelKind, ChannelContainer<P>>;
}
