use lightyear_serde::Serde;
use std::ops::{Deref, DerefMut};
use crate::shared::Components;

pub type PacketIndex = u16;
pub type Tick = u16;
pub type MessageIndex = u16;
pub type ShortMessageIndex = u8;
pub enum HostType {
    Server,
    Client,
}

/// ComponentId - should be one unique value for each type of Component
#[derive(Eq, Hash, Copy, Clone, PartialEq, Serde)]
pub struct ComponentId {
    inner: u16,
}

impl ComponentId {
    pub fn new(value: u16) -> Self {
        Self { inner: value }
    }

    pub fn name(&self) -> String {
        Components::id_to_name(self)
    }
}

impl Deref for ComponentId {
    type Target = u16;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for ComponentId {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

// MessageId
#[derive(Eq, Hash, Copy, PartialEq, Clone, Serde)]
pub struct MessageId {
    inner: u16,
}
impl MessageId {
    pub fn new(value: u16) -> Self {
        Self { inner: value }
    }
}
impl Deref for MessageId {
    type Target = u16;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for MessageId {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

// ChannelId
#[derive(Eq, Hash, Copy, PartialEq, Clone, Serde)]
pub struct ChannelId {
    inner: u16,
}
impl ChannelId {
    pub fn new(value: u16) -> Self {
        Self { inner: value }
    }
}
impl Deref for ChannelId {
    type Target = u16;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for ChannelId {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
