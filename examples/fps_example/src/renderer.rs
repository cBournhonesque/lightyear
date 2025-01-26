use crate::protocol::*;
use crate::shared::BOT_RADIUS;
use avian2d::prelude::*;
use bevy::color::palettes::basic::GREEN;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy::render::primitives::Aabb;
use bevy::render::RenderPlugin;
use lightyear::client::components::Confirmed;
use lightyear::prelude::client::{Interpolated, InterpolationSet, Predicted, PredictionSet};
use lightyear::prelude::server::ReplicationTarget;
use lightyear::transport::io::IoDiagnosticsPlugin;
use lightyear_avian::prelude::AabbEnvelopeHolder;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // draw after interpolation is done
        app.add_observer(add_bot_visuals);
        app.add_systems(
            PostUpdate,
            (
                #[cfg(feature = "server")]
                draw_aabb_envelope,
                draw_elements
                    .after(InterpolationSet::Interpolate)
                    .after(PredictionSet::VisualCorrection),
            ),
        );
        // TODO: draw bounding boxes for server aabb envelope
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Transform, &ColorComponent), (Without<Confirmed>, With<PlayerId>)>,
    balls: Query<(&Transform, &ColorComponent), (Without<Confirmed>, With<BulletMarker>)>,
) {
    for (transform, color) in &players {
        gizmos.rect_2d(
            Isometry2d::new(
                transform.translation.truncate(),
                transform.rotation.to_euler(EulerRot::XYZ).2.into(),
            ),
            Vec2::ONE * PLAYER_SIZE,
            color.0,
        );
    }
    for (transform, color) in &balls {
        gizmos.circle_2d(transform.translation.truncate(), BALL_SIZE, color.0);
    }
}

#[cfg(feature = "server")]
fn draw_aabb_envelope(
    query: Query<&ColliderAabb, With<AabbEnvelopeHolder>>,
    mut gizmos: Gizmos,
) {
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
    a: Or<(With<Predicted>, With<Interpolated>, With<ReplicationTarget>)>,
}

/// Add visuals to newly spawned bots
fn add_bot_visuals(
    trigger: Trigger<OnAdd, BotMarker>,
    query: Query<(), VisibleFilter>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let entity = trigger.entity();
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
