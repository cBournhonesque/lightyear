//! Module related to the concept of `Authority`
//!
//! A peer is said to have authority over an entity if it has the burden of simulating the entity.
//! Note that replicating state to other peers doesn't necessary mean that you have authority:
//! client C1 could have authority (is simulating the entity), replicated to the server which then replicates to other clients.
//! In this case C1 has authority even though the server is still replicating some states.
//!
use crate::prelude::ClientId;
use bevy::prelude::*;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Authority;

pub enum Peer {
    Server,
    Client(ClientId),
}

pub struct AuthorityRequest {
    pub entity: Entity,
    pub peer: Peer,
}

pub struct AuthorityResponse {
    success: bool,
}
