//! Module related to the concept of `Authority`
//!
//! A peer is said to have authority over an entity if it has the burden of simulating the entity.
//! Note that replicating state to other peers doesn't necessary mean that you have authority:
//! client C1 could have authority (is simulating the entity), replicated to the server which then replicates to other clients.
//! In this case C1 has authority even though the server is still replicating some states.
//!

use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear_connection::id::PeerId;
use serde::{Deserialize, Serialize};

/// Authority is used to define who is in charge of simulating an entity.
///
/// In particular:
/// - a client with Authority won't accept replication updates from the server
/// - a client without Authority won't be sending any replication updates
/// - a server won't accept replication updates from clients without Authority
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub struct HasAuthority;

#[derive(
    Component, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Reflect,
)]
#[reflect(Component)]
pub enum AuthorityPeer {
    None,
    #[default]
    Server,
    Client(PeerId),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthorityChange {
    pub entity: Entity,
    pub gain_authority: bool,
    /// Should we add prediction for that entity? This can be useful if the entity was originally
    /// spawned by a client C1, and then the authority was transferred away from that client.
    /// Now we want to start predicting the entity on client C1, but we cannot just rely on the normal
    /// systems because the entity already exists, and ShouldBePredicted only gets sent on the initial Spawn message
    pub add_prediction: bool,
    pub add_interpolation: bool,
}

impl MapEntities for AuthorityChange {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.entity = entity_mapper.get_mapped(self.entity);
    }
}