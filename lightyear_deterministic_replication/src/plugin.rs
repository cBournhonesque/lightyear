use crate::Deterministic;
use bevy_app::{App, Plugin};
use lightyear_prediction::rollback::DeterministicPredicted;

pub struct DeterministicReplicationPlugin;

impl Plugin for DeterministicReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<DeterministicPredicted, Deterministic>();
    }
}
