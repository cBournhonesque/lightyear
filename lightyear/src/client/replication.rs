use crate::_reexport::ClientMarker;
use crate::client::config::ClientConfig;
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::connection::ConnectionManager;
use crate::client::sync::client_is_synced;
use crate::prelude::client::InterpolationDelay;
use crate::prelude::Protocol;
use crate::shared::replication::plugin::ReplicationPlugin;
use crate::shared::sets::InternalReplicationSet;

#[derive(Clone, Debug)]
pub struct ReplicationConfig {
    /// Set to true to enable replicating this client's entities to the server
    pub enable_send: bool,
    /// Set to true to enable receiving replication updates from the server
    pub enable_receive: bool,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            enable_send: false,
            enable_receive: true,
        }
    }
}

pub struct ClientReplicationPlugin<P: Protocol> {
    marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Default for ClientReplicationPlugin<P> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol> Plugin for ClientReplicationPlugin<P> {
    fn build(&self, app: &mut App) {
        let config = app.world.resource::<ClientConfig>();
        app
            // PLUGIN
            .add_plugins(ReplicationPlugin::<P, ConnectionManager<P>>::new(
                config.shared.tick.tick_duration,
                config.replication.enable_send,
                config.replication.enable_receive,
            ))
            // TODO: currently we only support pre-spawned entities spawned during the FixedUpdate schedule
            // // SYSTEM SETS
            // .configure_sets(
            //     PostUpdate,
            //     // on client, the client hash component is not replicated to the server, so there's no ordering constraint
            //     ReplicationSet::SetPreSpawnedHash.in_set(ReplicationSet::All),
            // )
            .configure_sets(
                PostUpdate,
                // only replicate entities once client is synced
                // NOTE: we need is_synced, and not connected. Otherwise the ticks associated with the messages might be incorrect
                //  and the message might be ignored by the server
                //  But then pre-predicted entities that are spawned right away will not be replicated?
                // NOTE: we always need to add this condition if we don't enable replication, because
                InternalReplicationSet::<ClientMarker>::All.run_if(client_is_synced::<P>),
            );
    }
}
