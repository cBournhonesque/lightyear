use crate::_reexport::ClientMarker;
use crate::prelude::client::PredictionSet;
use crate::prelude::{ClientId, ClientMetadata, MainSet, Protocol};
use crate::shared::replication::components::Replicate;
use bevy::prelude::*;

pub(crate) struct MetadataPlugin<P> {
    marker: std::marker::PhantomData<P>,
}

impl<P> Default for MetadataPlugin<P> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol> Plugin for MetadataPlugin<P> {
    fn build(&self, app: &mut App) {
        app.init_resource::<GlobalMetadata>().add_systems(
            PreUpdate,
            update_client_id::<P>
                .after(MainSet::<ClientMarker>::ReceiveFlush)
                // SpawnPrediction uses the metadata to compare the client_id in ShouldBePredicted with the client_id in GlobalMetadata
                .before(PredictionSet::SpawnPrediction),
        );
    }
}

#[derive(Default, Resource)]
pub struct GlobalMetadata {
    /// The ClientId of the client from the server's point of view.
    /// Will be None if the client is not connected.
    pub client_id: Option<ClientId>,
}

// TODO: only run this if the client id is not known?
fn update_client_id<P: Protocol>(
    mut metadata: ResMut<GlobalMetadata>,
    // we add a Without<Replicate> bound for the unified mode where the client/server plugins
    // are running in the same app
    query: Query<&ClientMetadata, (Added<ClientMetadata>, Without<Replicate<P>>)>,
) {
    if let Ok(client_metadata) = query.get_single() {
        metadata.client_id = Some(client_metadata.client_id);
    }
}
