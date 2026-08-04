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

/// Certifies that this Link's [`RemoteId`](lightyear_core::id::RemoteId) was authenticated by its
/// connection backend.
///
/// This marker does not perform authentication. Secure connection backends such as the future
/// Iroh integration insert it only after cryptographically binding the Link to its remote public
/// identity. Raw transports must not insert it merely because a configured `RemoteId` exists.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub struct AuthenticatedPeerId;
