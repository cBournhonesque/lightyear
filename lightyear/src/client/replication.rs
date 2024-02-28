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
            .add_plugins(metadata::MetadataPlugin::default())
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

pub mod metadata {
    use crate::prelude::{ClientId, ClientMetadata, MainSet};
    use bevy::prelude::*;

    #[derive(Default)]
    pub(crate) struct MetadataPlugin;

    impl Plugin for MetadataPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<GlobalMetadata>()
                .add_systems(PreUpdate, update_client_id.after(MainSet::ReceiveFlush));
        }
    }

    #[derive(Default, Resource)]
    pub struct GlobalMetadata {
        /// The ClientId of the client from the server's point of view.
        /// Will be None if the client is not connected.
        pub client_id: Option<ClientId>,
    }

    fn update_client_id(
        mut metadata: ResMut<GlobalMetadata>,
        query: Query<&ClientMetadata, Added<ClientMetadata>>,
    ) {
        if let Ok(client_metadata) = query.get_single() {
            metadata.client_id = Some(client_metadata.client_id);
        }
    }
}
