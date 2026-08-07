use crate::client::Client;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;

/// Marks a client link as a direct connection to another peer in a P2P session.
///
/// P2P links remain [`Client`] links so that they can reuse the existing connection,
/// messaging, input, and prediction pipelines.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
#[require(Client)]
pub struct P2P;
