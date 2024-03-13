use crate::prelude::client::PredictionSet;
use crate::prelude::{ClientId, ClientMetadata, MainSet};
use bevy::prelude::*;

#[derive(Default)]
pub(crate) struct MetadataPlugin;

impl Plugin for MetadataPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GlobalMetadata>().add_systems(
            PreUpdate,
            update_client_id
                .after(MainSet::ReceiveFlush)
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

fn update_client_id(
    mut metadata: ResMut<GlobalMetadata>,
    query: Query<&ClientMetadata, Added<ClientMetadata>>,
) {
    if let Ok(client_metadata) = query.get_single() {
        metadata.client_id = Some(client_metadata.client_id);
    }
}
