use avian2d::parry::shape::Ball;
use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::rollback::{DeterministicPredicted, DisableRollback};
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{SharedPlugin, color_from_id, player_bundle, shared_movement_behaviour};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(handle_predicted_player);
    }
}

fn handle_predicted_player(
    trigger: On<Add, PlayerId>,
    client: Single<&LocalId, With<Client>>,
    timeline: Res<LocalTimeline>,
    mut commands: Commands,
    player_query: Query<&PlayerId>,
) {
    let mut entity_mut = commands.entity(trigger.entity);

    // entity_mut.insert((
    //
    //
    // ))
}
