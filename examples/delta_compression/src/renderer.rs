use bevy::prelude::*;

use crate::protocol::*;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(Update, draw_trails);
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

pub(crate) fn draw_trails(mut gizmos: Gizmos, trails: Query<(&PlayerTrail, &PlayerColor)>) {
    for (trail, color) in &trails {
        let len = trail.0.len();
        if len == 0 {
            continue;
        }
        for (index, point) in trail.0.iter().enumerate() {
            let fade = if len == 1 {
                1.0
            } else {
                1.0 - (index as f32 / (len - 1) as f32)
            };
            let color = color.0.with_alpha(fade * 0.8);
            let radius = if index == 0 { 8.0 } else { 4.0 };
            gizmos.circle_2d(point.0, radius, color);
            if let Some(next) = trail.0.get(index + 1) {
                gizmos.line_2d(point.0, next.0, color);
            }
        }
    }
}
