use crate::{
    protocol::{BlockMarker, CharacterMarker, ColorComponent, FloorMarker, ProjectileMarker},
    shared::{
        BLOCK_HEIGHT, BLOCK_WIDTH, CHARACTER_CAPSULE_HEIGHT, CHARACTER_CAPSULE_RADIUS,
        FLOOR_HEIGHT, FLOOR_WIDTH,
    },
};
use avian3d::{math::AsF32, prelude::*};
use bevy::{color::palettes::css::MAGENTA, prelude::*};
use lightyear::{
    client::prediction::diagnostics::PredictionDiagnosticsPlugin,
    prelude::{client::*, *},
    transport::io::IoDiagnosticsPlugin,
};
use lightyear::{
    client::prediction::rollback::DisableRollback, prelude::server::ReplicateToClient,
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

        // This is to test a setup where:
        // - enemies are interpolated
        // - they spawn Predicted bullets
        // - we use ReplicateOnce and DisableRollback to stop replicating any packets for these bullets
        app.add_systems(
            PreUpdate,
            (add_projectile_cosmetics)
                .after(PredictionSet::Sync)
                .before(PredictionSet::CheckRollback),
        );

        app.add_systems(
            PostUpdate,
            position_to_transform_for_interpolated.before(TransformSystem::TransformPropagate),
        );

        // Set up visual interp plugins for Transform. Transform is updated in FixedUpdate
        // by the physics plugin so we make sure that in PostUpdate we interpolate it
        app.add_plugins(VisualInterpolationPlugin::<Transform>::default());

        // Observers that add VisualInterpolationStatus components to entities
        // which receive a Position and are predicted
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
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0),
    ));
}

type ParentComponents = (
    &'static GlobalTransform,
    Option<&'static Position>,
    Option<&'static Rotation>,
);

type PosToTransformComponents = (
    &'static mut Transform,
    &'static Position,
    &'static Rotation,
    Option<&'static ChildOf>,
);

// Avian's sync plugin only runs for entities with RigidBody, but we want to also apply it for interpolated entities
pub fn position_to_transform_for_interpolated(
    mut query: Query<PosToTransformComponents, With<Interpolated>>,
    parents: Query<ParentComponents, With<Children>>,
) {
    for (mut transform, pos, rot, parent) in &mut query {
        if let Some(parent) = parent {
            if let Ok((parent_transform, parent_pos, parent_rot)) = parents.get(parent.parent()) {
                let parent_transform = parent_transform.compute_transform();
                let parent_pos = parent_pos.map_or(parent_transform.translation, |pos| pos.f32());
                let parent_rot = parent_rot.map_or(parent_transform.rotation, |rot| rot.f32());
                let parent_scale = parent_transform.scale;
                let parent_transform = Transform::from_translation(parent_pos)
                    .with_rotation(parent_rot)
                    .with_scale(parent_scale);

                let new_transform = GlobalTransform::from(
                    Transform::from_translation(pos.f32()).with_rotation(rot.f32()),
                )
                .reparented_to(&GlobalTransform::from(parent_transform));

                transform.translation = new_transform.translation;
                transform.rotation = new_transform.rotation;
            }
        } else {
            transform.translation = pos.f32();
            transform.rotation = rot.f32();
        }
    }
}

/// Add the VisualInterpolateStatus::<Transform> component to non-floor entities with
/// component `Position`. Floors don't need to be visually interpolated because we
/// don't expect them to move.
///
/// We query Without<Confirmed> instead of With<Predicted> so that the server's
/// gui will also get some visual interpolation. But we're usually just
/// concerned that the client's Predicted entities get the interpolation
/// treatment.
fn add_visual_interpolation_components(
    // We use Position because it's added by avian later, and when it's added
    // we know that Predicted is already present on the entity
    trigger: Trigger<OnAdd, Position>,
    query: Query<Entity, (With<Predicted>, Without<FloorMarker>)>,
    mut commands: Commands,
) {
    if !query.contains(trigger.target()) {
        return;
    }
    commands
        .entity(trigger.target())
        .insert(VisualInterpolateStatus::<Transform> {
            // We must trigger change detection on visual interpolation
            // to make sure that child entities (sprites, meshes, text)
            // are also interpolated
            trigger_change_detection: true,
            ..default()
        });
}

/// Add components to characters that impact how they are rendered. We only
/// want to see the predicted character and not the confirmed character.
fn add_character_cosmetics(
    mut commands: Commands,
    character_query: Query<
        (Entity, &ColorComponent),
        (
            Or<(
                Added<Predicted>,
                Added<ReplicateToClient>,
                Added<Interpolated>,
            )>,
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
        (Entity),
        (
            Or<(Added<Predicted>, Added<ReplicationMarker>)>,
            With<ProjectileMarker>,
        ),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity) in &character_query {
        info!(?entity, "Adding cosmetics to character {:?}", entity);
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Sphere::new(1.))),
            MeshMaterial3d(materials.add(Color::from(MAGENTA))),
            RigidBody::Dynamic, // needed to add this somewhere, lol
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
    floor_query: Query<
        Entity,
        (
            Or<(Added<Predicted>, Added<ReplicateToClient>)>,
            With<BlockMarker>,
        ),
    >,
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
            Or<(With<ProjectileMarker>, With<CharacterMarker>)>, // disabling character rollbacks while we debug projectiles with this janky setup
            Without<DisableRollback>,
        ),
    >,
) {
    for proj in &q_projectile {
        commands.entity(proj).insert(DisableRollback);
    }
}
