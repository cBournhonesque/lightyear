use crate::protocol::*;
use crate::shared::BOT_RADIUS;
use avian2d::prelude::*;
use bevy::color::palettes::basic::GREEN;
use bevy::color::palettes::css::BLUE;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use lightyear::interpolation::Interpolated;
use lightyear::prelude::{PreSpawned, Predicted, Replicate, Replicated};
use lightyear_avian2d::prelude::AabbEnvelopeHolder;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_observer(add_interpolated_bot_visuals);
        app.add_observer(add_predicted_bot_visuals);
        app.add_observer(add_bullet_visuals);
        app.add_observer(add_player_visuals);
        app.add_plugins(FrameInterpolationPlugin::<Transform>::default());

        #[cfg(feature = "client")]
        {
            app.add_systems(Update, display_score);
        }

        #[cfg(feature = "server")]
        {
            app.add_systems(PostUpdate, draw_aabb_envelope);
        }
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
    #[cfg(feature = "client")]
    commands.spawn((
        Text::new("Score: 0"),
        TextFont::from_font_size(30.0),
        TextColor(Color::WHITE.with_alpha(0.5)),
        Node {
            align_self: AlignSelf::End,
            ..default()
        },
        ScoreText,
    ));
}

#[derive(Component)]
struct ScoreText;

#[cfg(feature = "client")]
fn display_score(
    mut score_text: Query<&mut Text, With<ScoreText>>,
    hits: Query<&Score, With<Replicated>>,
) {
    if let Ok(score) = hits.single() {
        if let Ok(mut text) = score_text.single_mut() {
            text.0 = format!("Score: {}", score.0);
        }
    }
}

#[cfg(feature = "server")]
fn draw_aabb_envelope(query: Query<&ColliderAabb, With<AabbEnvelopeHolder>>, mut gizmos: Gizmos) {
    query.iter().for_each(|collider_aabb| {
        gizmos.rect_2d(
            Isometry2d::new(collider_aabb.center(), Rot2::default()),
            collider_aabb.size(),
            Color::WHITE,
        );
    })
}

// TODO: interpolated players are not visible because components are not inserted at the same time?
/// Add visuals to newly spawned players
fn add_player_visuals(
    trigger: On<Add, PlayerId>,
    query: Query<(Has<Predicted>, &ColorComponent), Without<BulletMarker>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((is_predicted, color)) = query.get(trigger.entity) {
        commands.entity(trigger.entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Rectangle::from_length(PLAYER_SIZE)))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if is_predicted {
            commands
                .entity(trigger.entity)
                .insert(FrameInterpolate::<Transform>::default());
        }
    }
}

/// Add visuals to newly spawned bullets
fn add_bullet_visuals(
    trigger: On<Add, BulletMarker>,
    query: Query<(&ColorComponent, Has<Interpolated>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, interpolated)) = query.get(trigger.entity) {
        commands.entity(trigger.entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle {
                radius: BULLET_SIZE,
            }))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if interpolated {
            commands
                .entity(trigger.entity)
                .insert(FrameInterpolate::<Transform>::default());
        }
    }
}

/// Add visuals to newly spawned bots
fn add_interpolated_bot_visuals(
    trigger: On<Add, InterpolatedBot>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let entity = trigger.entity;
    // add visibility
    commands.entity(entity).insert((
        Visibility::default(),
        Mesh2d(meshes.add(Mesh::from(Circle { radius: BOT_RADIUS }))),
        MeshMaterial2d(materials.add(ColorMaterial {
            color: GREEN.into(),
            ..Default::default()
        })),
    ));
}

fn add_predicted_bot_visuals(
    trigger: On<Add, PredictedBot>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let entity = trigger.entity;
    // add visibility
    commands.entity(entity).insert((
        Visibility::default(),
        Mesh2d(meshes.add(Mesh::from(Circle { radius: BOT_RADIUS }))),
        MeshMaterial2d(materials.add(ColorMaterial {
            color: BLUE.into(),
            ..Default::default()
        })),
        // predicted entities are updated in FixedUpdate so they need to be visually interpolated
        FrameInterpolate::<Transform>::default(),
    ));
}
