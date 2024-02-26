use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::connection::ConnectionManager;
use crate::client::sync::client_is_synced;
use crate::prelude::{Protocol, ReplicationSet};
use crate::shared::replication::plugin::ReplicationPlugin;

pub struct ClientReplicationPlugin<P: Protocol> {
    tick_duration: Duration,
    marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> ClientReplicationPlugin<P> {
    pub(crate) fn new(tick_duration: Duration) -> Self {
        Self {
            tick_duration,
            marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol> Plugin for ClientReplicationPlugin<P> {
    fn build(&self, app: &mut App) {
        app
            // PLUGIN
            .add_plugins(ReplicationPlugin::<P, ConnectionManager<P>>::new(
                self.tick_duration,
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
                ReplicationSet::All.run_if(client_is_synced::<P>),
            );
    }
}
