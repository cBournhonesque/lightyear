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
        app.add_observer(add_bot_visuals);
        app.add_observer(add_bullet_visuals);
        app.add_observer(add_player_visuals);

        app.add_systems(
            PostUpdate,
            (
                #[cfg(feature = "server")]
                draw_aabb_envelope,
                // // draw after interpolation is done
                // draw_elements
                //     .after(InterpolationSet::Interpolate)
                //     .after(PredictionSet::VisualCorrection),
            ),
        );
        // TODO: draw bounding boxes for server aabb envelope
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
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

/// Add visuals to newly spawned players
fn add_player_visuals(
    trigger: Trigger<OnAdd, PlayerId>,
    query: Query<&ColorComponent, VisibleFilter>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let entity = trigger.entity();
    if let Ok(color) = query.get(entity) {
        // add visibility
        commands.entity(entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Rectangle::from_length(PLAYER_SIZE)))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
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
    let entity = trigger.entity();
    if let Ok(color) = query.get(entity) {
        // add visibility
        commands.entity(entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle { radius: BULLET_SIZE}))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
    }
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
