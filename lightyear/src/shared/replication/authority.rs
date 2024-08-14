//! Module related to the concept of `Authority`
//!
//! A peer is said to have authority over an entity if it has the burden of simulating the entity.
//! Note that replicating state to other peers doesn't necessary mean that you have authority:
//! client C1 could have authority (is simulating the entity), replicated to the server which then replicates to other clients.
//! In this case C1 has authority even though the server is still replicating some states.
//!

use crate::prelude::{ClientId, Deserialize, Serialize};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;

/// Authority is used to define who is in charge of simulating an entity.
///
/// In particular:
/// - a client with Authority won't accept replication updates from the server
/// - a client without Authority won't be sending any replication updates
/// - a server won't accept replication updates from clients without Authority
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HasAuthority;

#[derive(Component, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityPeer {
    None,
    Server,
    Client(ClientId),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferAuthority {
    pub entity: Entity,
    pub peer: AuthorityPeer,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthorityChange {
    pub entity: Entity,
    pub gain_authority: bool,
}

impl MapEntities for AuthorityChange {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.entity = entity_mapper.map_entity(self.entity);
    }
}

pub(crate) struct GetAuthorityRequest {
    pub entity: Entity,
}

pub(crate) struct GetAuthorityResponse {
    success: bool,
}
