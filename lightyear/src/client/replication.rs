//! Client replication plugins
use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::networking::is_connected;
use crate::client::sync::client_is_synced;
use crate::prelude::client::InterpolationDelay;
use crate::prelude::SharedConfig;
use crate::shared::replication::plugin::receive::ReplicationReceivePlugin;
use crate::shared::replication::plugin::send::ReplicationSendPlugin;
use crate::shared::sets::{ClientMarker, InternalReplicationSet};

#[derive(Default)]
pub struct ClientReplicationReceivePlugin {
    pub tick_interval: Duration,
}

impl Plugin for ClientReplicationReceivePlugin {
    fn build(&self, app: &mut App) {
        // PLUGIN
        app.add_plugins(ReplicationReceivePlugin::<ConnectionManager>::new(
            self.tick_interval,
        ));

        // TODO: currently we only support pre-spawned entities spawned during the FixedUpdate schedule
        // // SYSTEM SETS
        // .configure_sets(
        //     PostUpdate,
        //     // on client, the client hash component is not replicated to the server, so there's no ordering constraint
        //     ReplicationSet::SetPreSpawnedHash.in_set(ReplicationSet::All),
        // )

        app.configure_sets(
            PostUpdate,
            // only replicate entities once client is synced
            // NOTE: we need is_synced, and not connected. Otherwise the ticks associated with the messages might be incorrect
            //  and the message might be ignored by the server
            //  But then pre-predicted entities that are spawned right away will not be replicated?
            // NOTE: we always need to add this condition if we don't enable replication, because
            InternalReplicationSet::<ClientMarker>::All.run_if(
                is_connected
                    .and_then(client_is_synced)
                    .and_then(not(SharedConfig::is_host_server_condition)),
            ),
        );
    }
}

#[derive(Default)]
pub struct ClientReplicationSendPlugin {
    pub tick_interval: Duration,
}

impl Plugin for ClientReplicationSendPlugin {
    fn build(&self, app: &mut App) {
        app
            // PLUGIN
            .add_plugins(ReplicationSendPlugin::<ConnectionManager>::new(
                self.tick_interval,
            ))
            // SETS
            .configure_sets(
                PostUpdate,
                // only replicate entities once client is synced
                // NOTE: we need is_synced, and not connected. Otherwise the ticks associated with the messages might be incorrect
                //  and the message might be ignored by the server
                //  But then pre-predicted entities that are spawned right away will not be replicated?
                // NOTE: we always need to add this condition if we don't enable replication, because
                InternalReplicationSet::<ClientMarker>::All.run_if(
                    is_connected
                        .and_then(client_is_synced)
                        .and_then(not(SharedConfig::is_host_server_condition)),
                ),
            );
    }
}
