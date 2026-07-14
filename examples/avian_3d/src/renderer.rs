use crate::{
    protocol::{BlockMarker, CharacterMarker, ColorComponent, FloorMarker, ProjectileMarker},
    shared::{
        ProjectilePhysicsBundle, BLOCK_HEIGHT, BLOCK_WIDTH, CHARACTER_CAPSULE_HEIGHT,
        CHARACTER_CAPSULE_RADIUS, FLOOR_HEIGHT, FLOOR_WIDTH, PROJECTILE_RADIUS,
    },
};
use avian3d::{math::AsF32, prelude::*};
use bevy::{color::palettes::css::MAGENTA, prelude::*};
use lightyear::prediction::plugin::PredictionSystems;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::*;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

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

        // This is to test a setup where:
        // - enemies are interpolated
        // - they spawn Predicted bullets
        // - we use ReplicateOnce and DisableRollback to stop replicating any packets for these bullets
        app.add_systems(
            PreUpdate,
            add_projectile_cosmetics.before(RollbackSystems::Check),
        );

        // Position/Rotation are updated by physics in FixedUpdate, so frame-interpolate them in
        // PostUpdate for smooth rendering.
        if !app.is_plugin_added::<FrameInterpolationPlugin>() {
            app.add_plugins(FrameInterpolationPlugin);
        }

        // Add the type-erased FrameInterpolate marker to predicted entities with Position.
        app.add_observer(add_visual_interpolation_components);

        // We disable rollbacks for projectiles after the initial rollbacks which brings them to the predicted timeline
        app.add_systems(Last, disable_projectile_rollback);
    }
}

fn init(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 4.5, -9.0).looking_at(Vec3::ZERO, Dir3::Y),
    ));

    commands.spawn((
        PointLight {
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0),
    ));
}

/// Add the FrameInterpolate marker to non-floor entities with
/// component `Position`. Floors don't need to be frame interpolated because we
/// don't expect them to move.
fn add_visual_interpolation_components(
    // We use Position because it's added by avian later, and when it's added
    // we know that Predicted is already present on the entity
    trigger: On<Add, Position>,
    query: Query<Entity, (With<Predicted>, Without<FloorMarker>)>,
    clients: Query<(), With<Client>>,
    mut commands: Commands,
) {
    if clients.is_empty() {
        return;
    }
    if !query.contains(trigger.entity) {
        return;
    }
    commands.entity(trigger.entity).insert(FrameInterpolate);
}

/// Add components to characters that impact how they are rendered. We only
/// want to see the predicted character and not the confirmed character.
fn add_character_cosmetics(
    mut commands: Commands,
    character_query: Query<
        (Entity, &ColorComponent),
        (
            Or<(Added<Predicted>, Added<Replicate>, Added<Interpolated>)>,
            With<CharacterMarker>,
        ),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, color) in &character_query {
        info!(?entity, "Adding cosmetics to character {:?}", entity);
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Capsule3d::new(
                CHARACTER_CAPSULE_RADIUS,
                CHARACTER_CAPSULE_HEIGHT,
            ))),
            MeshMaterial3d(materials.add(color.0)),
        ));
    }
}

fn add_projectile_cosmetics(
    mut commands: Commands,
    character_query: Query<
        Entity,
        (
            Or<(Added<Predicted>, Added<Replicate>)>,
            With<ProjectileMarker>,
        ),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for entity in &character_query {
        info!(?entity, "Adding cosmetics to projectile {:?}", entity);
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Sphere::new(PROJECTILE_RADIUS))),
            MeshMaterial3d(materials.add(Color::from(MAGENTA))),
            ProjectilePhysicsBundle::default(),
        ));
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
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Cuboid::new(FLOOR_WIDTH, FLOOR_HEIGHT, FLOOR_WIDTH))),
            MeshMaterial3d(materials.add(Color::srgb(1.0, 1.0, 1.0))),
        ));
    }
}

/// Add components to blocks that impact how they are rendered. We only want to
/// see the predicted block and not the confirmed block.
fn add_block_cosmetics(
    mut commands: Commands,
    floor_query: Query<Entity, (Or<(Added<Predicted>, Added<Replicate>)>, With<BlockMarker>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for entity in &floor_query {
        info!(?entity, "Adding cosmetics to block {:?}", entity);
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Cuboid::new(BLOCK_WIDTH, BLOCK_HEIGHT, BLOCK_WIDTH))),
            MeshMaterial3d(materials.add(Color::srgb(1.0, 0.0, 1.0))),
        ));
    }
}

fn disable_projectile_rollback(
    mut commands: Commands,
    q_projectile: Query<
        Entity,
        (
            With<Predicted>,
            With<ProjectileMarker>,
            // Or<(With<ProjectileMarker>, With<CharacterMarker>)>,
            // disabling character rollbacks while we debug projectiles with this janky setup

            // We stop checking for state rollbacks after the first frame where the projectile is predicted
            Without<DisableRollback>,
        ),
    >,
) {
    for proj in &q_projectile {
        commands.entity(proj).insert(DisableRollback);
    }
}
