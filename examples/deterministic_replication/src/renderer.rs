use crate::protocol::*;
use crate::shared::Wall;
use avian2d::prelude::{Position, Rotation};
use bevy::color::palettes::css;
use bevy::prelude::*;
use lightyear::prelude::{InterpolationSystems, RollbackSystems};
use lightyear_deterministic_replication::prelude::CatchUpMode;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(Update, update_mode_text);
        // draw after interpolation is done
        app.add_systems(
            PostUpdate,
            draw_elements
                .after(InterpolationSystems::Interpolate)
                .after(RollbackSystems::VisualCorrection),
        );
        // Frame interpolation is registered in SharedPlugin because visual
        // correction shares its marker, history, and restore pipeline.
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        Text::new("Catch-up: StateBasedCatchUp"),
        TextFont::from_font_size(20.0),
        TextColor(Color::WHITE.with_alpha(0.8)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            left: Val::Px(10.0),
            ..default()
        },
        ModeText,
    ));
}

#[derive(Component)]
struct ModeText;

fn update_mode_text(mode: Res<CatchUpMode>, mut text: Single<&mut Text, With<ModeText>>) {
    let label = match *mode {
        CatchUpMode::InputOnly => "InputOnly",
        CatchUpMode::StateBasedCatchUp => "StateBasedCatchUp",
    };
    text.0 = format!("Catch-up: {label}");
}

/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Position, &Rotation, &ColorComponent), With<PlayerId>>,
    balls: Query<(&Position, &ColorComponent), With<BallMarker>>,
    ball_parts: Query<(&GlobalTransform, &BallPart)>,
    walls: Query<(&Wall, &ColorComponent), Without<PlayerId>>,
) {
    for (position, rotation, color) in &players {
        gizmos.rect_2d(
            Isometry2d {
                rotation: Rot2 {
                    sin: rotation.sin,
                    cos: rotation.cos,
                },
                translation: Vec2::new(position.x, position.y),
            },
            Vec2::ONE * PLAYER_SIZE,
            color.0,
        );
    }
    for (position, color) in &balls {
        gizmos.circle_2d(
            Vec2::new(position.x, position.y),
            BALL_SIZE,
            color.0.with_alpha(0.18),
        );
    }
    for (global, part) in &ball_parts {
        let position = global.translation().truncate();
        match part {
            BallPart::Core => {
                gizmos.circle_2d(position, 12.0, css::AZURE);
            }
            BallPart::Lobe => {
                gizmos.circle_2d(position, 8.0, css::AZURE);
            }
            BallPart::Sensor => {
                gizmos.circle_2d(position, 20.0, css::AZURE.with_alpha(0.25));
            }
            BallPart::Pivot => {}
        }
    }
    for (wall, color) in &walls {
        gizmos.line_2d(wall.start, wall.end, color.0);
    }
}
