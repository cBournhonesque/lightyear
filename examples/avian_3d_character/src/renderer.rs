use avian3d::prelude::*;
use bevy::prelude::*;
use bevy_screen_diagnostics::{
    Aggregate, ScreenDiagnostics, ScreenDiagnosticsPlugin, ScreenEntityDiagnosticsPlugin,
};
use lightyear::{
    client::prediction::diagnostics::PredictionDiagnosticsPlugin,
    prelude::{client::*, *},
    transport::io::IoDiagnosticsPlugin,
};

use crate::{
    protocol::{BlockMarker, CharacterMarker, ColorComponent, FloorMarker},
    shared::{
        BLOCK_HEIGHT, BLOCK_WIDTH, CHARACTER_CAPSULE_HEIGHT, CHARACTER_CAPSULE_RADIUS,
        FLOOR_HEIGHT, FLOOR_WIDTH,
    },
};

pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(
            Update,
            (
                add_character_cosmetics,
                add_floor_cosmetics,
                add_block_cosmetics,
            ),
        );

        app.add_plugins(ScreenDiagnosticsPlugin::default());
        app.add_plugins(ScreenEntityDiagnosticsPlugin);

        // Set up visual interp plugins for Position and Rotation. This doesn't
        // do anything until you add VisualInterpolationStatus components to
        // entities.
        app.add_plugins(VisualInterpolationPlugin::<Position>::default());
        app.add_plugins(VisualInterpolationPlugin::<Rotation>::default());

        // Observers that add VisualInterpolationStatus components to entities
        // which receive a Position or Rotation component.
        app.observe(add_visual_interpolation_components::<Position>);
        app.observe(add_visual_interpolation_components::<Rotation>);
    }
}

fn init(mut commands: Commands, mut onscreen: ResMut<ScreenDiagnostics>) {
    commands.spawn(Camera3dBundle {
        transform: Transform::from_xyz(0.0, 4.5, -9.0).looking_at(Vec3::ZERO, Dir3::Y),
        ..default()
    });

    commands.spawn(PointLightBundle {
        point_light: PointLight {
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::from_xyz(4.0, 8.0, 4.0),
        ..default()
    });

    onscreen
        .add("RB".to_string(), PredictionDiagnosticsPlugin::ROLLBACKS)
        .aggregate(Aggregate::Value)
        .format(|v| format!("{v:.0}"));
    onscreen
        .add(
            "RBt".to_string(),
            PredictionDiagnosticsPlugin::ROLLBACK_TICKS,
        )
        .aggregate(Aggregate::Value)
        .format(|v| format!("{v:.0}"));
    onscreen
        .add(
            "RBd".to_string(),
            PredictionDiagnosticsPlugin::ROLLBACK_DEPTH,
        )
        .aggregate(Aggregate::Value)
        .format(|v| format!("{v:.1}"));
    // Screen diagnostics twitches due to layout change when a metric adds or
    // removes a digit so pad these metrics to 3 digits.
    onscreen
        .add("KB_in".to_string(), IoDiagnosticsPlugin::BYTES_IN)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:0>3.0}"));
    onscreen
        .add("KB_out".to_string(), IoDiagnosticsPlugin::BYTES_OUT)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:0>3.0}"));
}

/// Add the VisualInterpolateStatus component to non-floor entities with
/// component `T`. Floors don't need to be visually interpolated because we
/// don't expect them to move.
///
/// We query Without<Confirmed> instead of With<Predicted> so that the server's
/// gui will also get some visual interpolation. But we're usually just
/// concerned that the client's Predicted entities get the interpolation
/// treatment.
///
/// Make sure that avian's SyncPlugin is run in PostUpdate in order to
/// incorporate the changes in pos/rot due to visual interpolation. Entities
/// rendered based on transforms will then have transforms based on the visual
/// interpolation.
fn add_visual_interpolation_components<T: Component>(
    trigger: Trigger<OnAdd, T>,
    query: Query<Entity, (With<T>, Without<Confirmed>, Without<FloorMarker>)>,
    mut commands: Commands,
) {
    if !query.contains(trigger.entity()) {
        return;
    }
    debug!("Adding visual interp component to {:?}", trigger.entity());
    commands
        .entity(trigger.entity())
        .insert(VisualInterpolateStatus::<T> {
            // We must trigger change detection so that the SyncPlugin will
            // detect and sync changes from Position/Rotation to Transform.
            //
            // Without syncing interpolated pos/rot to transform, things like
            // sprites, meshes, and text which render based on the *Transform*
            // component (not avian's Position) will be stuttery.
            trigger_change_detection: true,
            ..default()
        });
}

/// Add components to characters that impact how they are rendered. We only
/// want to see the predicted character and not the confirmed character.
fn add_character_cosmetics(
    mut commands: Commands,
    character_query: Query<(Entity, &ColorComponent), (Added<Predicted>, With<CharacterMarker>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, color) in &character_query {
        info!(?entity, "Adding cosmetics to character {:?}", entity);
        commands.entity(entity).insert((PbrBundle {
            mesh: meshes.add(Capsule3d::new(
                CHARACTER_CAPSULE_RADIUS,
                CHARACTER_CAPSULE_HEIGHT,
            )),
            material: materials.add(color.0),
            ..default()
        },));
    }
}

/// Add components to floors that impact how they are rendered. We want to see
/// the replicated floor instead of predicted floors because predicted floors
/// do not exist since floors aren't predicted.
fn add_floor_cosmetics(
    mut commands: Commands,
    floor_query: Query<Entity, Added<FloorMarker>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for entity in &floor_query {
        info!(?entity, "Adding cosmetics to floor {:?}", entity);
        commands.entity(entity).insert(PbrBundle {
            mesh: meshes.add(Cuboid::new(FLOOR_WIDTH, FLOOR_HEIGHT, FLOOR_WIDTH)),
            material: materials.add(Color::srgb(1.0, 1.0, 1.0)),
            ..default()
        });
    }
}

/// Add components to blocks that impact how they are rendered. We only want to
/// see the predicted block and not the confirmed block.
fn add_block_cosmetics(
    mut commands: Commands,
    floor_query: Query<Entity, (Added<Predicted>, With<BlockMarker>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for entity in &floor_query {
        info!(?entity, "Adding cosmetics to block {:?}", entity);
        commands.entity(entity).insert(PbrBundle {
            mesh: meshes.add(Cuboid::new(BLOCK_WIDTH, BLOCK_HEIGHT, BLOCK_WIDTH)),
            material: materials.add(Color::srgb(1.0, 0.0, 1.0)),
            ..default()
        });
    }
}
