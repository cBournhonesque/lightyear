use crate::protocol::*;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy_egui::EguiPlugin;
use lightyear::connection::host::HostServer;
use lightyear::interpolation::Interpolated;
use lightyear::prediction::Predicted;
use lightyear::prelude::{
    lightyear_debug_event, Client, DebugCategory, DebugSamplePoint, InputTimeline,
    InterpolationTimeline, IsSynced,
};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(FixedPostUpdate, update_player_visual_positions);
        app.add_systems(
            PostUpdate,
            (ensure_player_visual_positions, draw_boxes).chain(),
        );
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

#[derive(Component, Debug, Clone, Copy)]
struct VisualPlayerPosition {
    previous: Option<Vec2>,
    current: Vec2,
}

fn client_visuals_ready(
    client: &Query<(), With<Client>>,
    host_server: &Query<(), With<HostServer>>,
    input_synced: &Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    interpolation_synced: &Query<(), (With<Client>, With<IsSynced<InterpolationTimeline>>)>,
) -> bool {
    client.is_empty()
        || !host_server.is_empty()
        || (!input_synced.is_empty() && !interpolation_synced.is_empty())
}

fn ensure_player_visual_positions(
    mut commands: Commands,
    players: Query<
        (Entity, &PlayerPosition),
        (
            With<PlayerId>,
            Or<(With<Predicted>, With<Interpolated>)>,
            Without<VisualPlayerPosition>,
        ),
    >,
    client: Query<(), With<Client>>,
    host_server: Query<(), With<HostServer>>,
    input_synced: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    interpolation_synced: Query<(), (With<Client>, With<IsSynced<InterpolationTimeline>>)>,
) {
    if !client_visuals_ready(&client, &host_server, &input_synced, &interpolation_synced) {
        return;
    }
    for (entity, position) in &players {
        commands.entity(entity).insert(VisualPlayerPosition {
            previous: None,
            current: position.0,
        });
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "lobby_player_visual_position_added",
            entity = ?entity,
            position = ?position,
            "Lobby player visual position added"
        );
    }
}

fn update_player_visual_positions(
    mut players: Query<
        (&PlayerPosition, &mut VisualPlayerPosition),
        Or<(With<Predicted>, With<Interpolated>)>,
    >,
) {
    for (position, mut visual) in &mut players {
        if visual.current != position.0 {
            let current = visual.current;
            visual.previous = Some(current);
            visual.current = position.0;
        }
    }
}

/// System that draws the boxes of the visually interpolated player positions.
fn draw_boxes(
    mut gizmos: Gizmos,
    players: Query<(
        Entity,
        &PlayerPosition,
        &VisualPlayerPosition,
        &PlayerColor,
        Has<Predicted>,
        Has<Interpolated>,
    )>,
    fixed_time: Res<Time<Fixed>>,
) {
    let overstep = fixed_time.overstep_fraction();
    for (entity, position, visual, color, is_predicted, is_interpolated) in &players {
        let visual_position = visual
            .previous
            .map(|previous| previous.lerp(visual.current, overstep))
            .unwrap_or(visual.current);
        debug!("Drawing player at {:?}", visual_position);
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "lobby_draw_player",
            entity = ?entity,
            position = ?position,
            visual_position = ?visual_position,
            is_predicted = is_predicted,
            is_interpolated = is_interpolated,
            "Lobby draw player"
        );
        gizmos.rect_2d(
            Isometry2d::from_translation(visual_position),
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}
