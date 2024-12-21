use crate::protocol::*;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy_screen_diagnostics::{Aggregate, ScreenDiagnostics};
use lightyear::client::components::Confirmed;
use lightyear::transport::io::IoDiagnosticsPlugin;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // draw after interpolation is done
        app.add_systems(
            PostUpdate,
            draw_elements
                .after(InterpolationSet::Interpolate)
                .after(PredictionSet::VisualCorrection),
        );
        app.add_systems(Startup, crate::shared::setup_diagnostic);
        app.add_plugins(ScreenDiagnosticsPlugin::default());
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn setup_diagnostic(mut onscreen: ResMut<ScreenDiagnostics>) {
    onscreen
        .add("KB/S in".to_string(), IoDiagnosticsPlugin::BYTES_IN)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.2}"));
    onscreen
        .add("KB/s out".to_string(), IoDiagnosticsPlugin::BYTES_OUT)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.2}"));
}

pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Transform, &ColorComponent), (Without<Confirmed>, With<PlayerId>)>,
    // // we will change the color of balls when they become predicted (i.e. adopt server authority)
    // prespawned_balls: Query<
    //     (&Transform, &ColorComponent),
    //     (
    //         With<PreSpawnedPlayerObject>,
    //         Without<Predicted>,
    //         With<BallMarker>,
    //     ),
    // >,
    // predicted_balls: Query<
    //     (&Transform, &ColorComponent),
    //     (
    //         Without<PreSpawnedPlayerObject>,
    //         With<Predicted>,
    //         With<BallMarker>,
    //     ),
    // >,
    balls: Query<(&Transform, &ColorComponent), (Without<Confirmed>, With<BallMarker>)>,
) {
    for (transform, color) in &players {
        // transform.rotation.angle_between()
        // let angle = transform.rotation.to_axis_angle().1;
        // warn!(axis = ?transform.rotation.to_axis_angle().0);
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
    // for (transform, color) in &prespawned_balls {
    //     let color = color.0.set
    //     gizmos.circle_2d(transform.translation.truncate(), BALL_SIZE, color.0);
    // }
}
