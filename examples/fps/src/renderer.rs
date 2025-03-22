use crate::protocol::*;
use crate::shared::BOT_RADIUS;
use avian2d::prelude::*;
use bevy::color::palettes::basic::GREEN;
use bevy::color::palettes::css::BLUE;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use lightyear::client::interpolation::VisualInterpolationPlugin;
use lightyear::prelude::client::{Interpolated, Predicted, VisualInterpolateStatus};
use lightyear::prelude::server::ReplicateToClient;
use lightyear::prelude::{NetworkIdentity, PreSpawned, Replicated};
use lightyear_avian::prelude::AabbEnvelopeHolder;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_observer(add_interpolated_bot_visuals);
        app.add_observer(add_predicted_bot_visuals);
        app.add_observer(add_bullet_visuals);
        app.add_observer(add_player_visuals);
        app.add_plugins(VisualInterpolationPlugin::<Transform>::default());

        #[cfg(feature = "client")]
        {
            app.add_systems(Startup, spawn_score_text);
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
}

#[derive(Component)]
struct ScoreText;

#[cfg(feature = "client")]
fn spawn_score_text(mut commands: Commands, identity: NetworkIdentity) {
    if identity.is_client() {
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
}

#[cfg(feature = "client")]
fn display_score(
    mut score_text: Query<&mut Text, With<ScoreText>>,
    hits: Query<&Score, With<Replicated>>,
) {
    if let Ok(score) = hits.get_single() {
        if let Ok(mut text) = score_text.get_single_mut() {
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

/// Convenient for filter for entities that should be visible
/// Works either on the client or the server
#[derive(QueryFilter)]
pub struct VisibleFilter {
    a: Or<(
        With<Predicted>,
        // to show prespawned entities
        With<PreSpawned>,
        With<Interpolated>,
        With<ReplicateToClient>,
    )>,
    // we don't show any replicated (confirmed) entities
    b: Without<Replicated>,
}

/// Add visuals to newly spawned players
fn add_player_visuals(
    trigger: Trigger<OnAdd, PlayerId>,
    query: Query<(Has<Predicted>, &ColorComponent), (VisibleFilter, Without<BulletMarker>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((is_predicted, color)) = query.get(trigger.target()) {
        commands.entity(trigger.target()).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Rectangle::from_length(PLAYER_SIZE)))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if is_predicted {
            commands
                .entity(trigger.target())
                .insert(VisualInterpolateStatus::<Transform>::default());
        }
    }
}

/// Add visuals to newly spawned bullets
fn add_bullet_visuals(
    trigger: Trigger<OnAdd, BulletMarker>,
    query: Query<&ColorComponent, VisibleFilter>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok(color) = query.get(trigger.target()) {
        commands.entity(trigger.target()).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle {
                radius: BULLET_SIZE,
            }))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
            VisualInterpolateStatus::<Transform>::default(),
        ));
    }
}

/// Add visuals to newly spawned bots
fn add_interpolated_bot_visuals(
    trigger: Trigger<OnAdd, InterpolatedBot>,
    query: Query<(), VisibleFilter>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let entity = trigger.target();
    if query.get(entity).is_ok() {
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
}

fn add_predicted_bot_visuals(
    trigger: Trigger<OnAdd, PredictedBot>,
    query: Query<(), VisibleFilter>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let entity = trigger.target();
    if query.get(entity).is_ok() {
        // add visibility
        commands.entity(entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle { radius: BOT_RADIUS }))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: BLUE.into(),
                ..Default::default()
            })),
            // predicted entities are updated in FixedUpdate so they need to be visually interpolated
            VisualInterpolateStatus::<Transform>::default(),
        ));
    }
}
