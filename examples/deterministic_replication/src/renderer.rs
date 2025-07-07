use crate::protocol::*;
use crate::shared::Wall;
use avian2d::position::{Position, Rotation};
use bevy::prelude::*;
use lightyear::prediction::Predicted;
use lightyear::prelude::{Client, Connected, InterpolationSet, RollbackSet, TriggerSender};
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

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
                .after(RollbackSet::VisualCorrection),
        );

        #[cfg(feature = "client")]
        {
            app.add_systems(Startup, ready_button);
        }

        // add visual interpolation for Position and Rotation
        // (normally we would interpolate on Transform but here this is fine
        // since rendering is done via Gizmos that only depend on Position/Rotation)
        app.add_plugins(FrameInterpolationPlugin::<Position>::default());
        app.add_plugins(FrameInterpolationPlugin::<Rotation>::default());
        app.add_observer(add_visual_interpolation_components);
    }
}

fn add_visual_interpolation_components(
    trigger: Trigger<OnAdd, Position>,
    predicted: Query<(), With<Predicted>>,
    mut commands: Commands,
) {
    if let Ok(()) = predicted.get(trigger.target()) {
        commands.entity(trigger.target()).insert((
            FrameInterpolate::<Position>::default(),
            FrameInterpolate::<Rotation>::default(),
        ));
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}


/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Position, &Rotation, &ColorComponent), With<PlayerId>>,
    balls: Query<(&Position, &ColorComponent), With<BallMarker>>,
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
        gizmos.circle_2d(Vec2::new(position.x, position.y), BALL_SIZE, color.0);
    }
    for (wall, color) in &walls {
        gizmos.line_2d(wall.start, wall.end, color.0);
    }
}

#[cfg(feature = "client")]
pub(crate) fn ready_button(
    mut commands: Commands,
) {
    commands
        .spawn((
            Text("Ready".to_string()),
            TextColor(Color::srgb(0.9, 0.9, 0.9)),
            TextFont::from_font_size(20.0),
            Button,
            BorderColor(Color::BLACK),
            Node {
                width: Val::Px(150.0),
                height: Val::Px(65.0),
                padding: UiRect::all(Val::Px(10.0)),
                border: UiRect::all(Val::Px(5.0)),
                position_type: PositionType::Absolute,
                right: Val::Percent(0.10),
                // horizontally center child text
                justify_content: JustifyContent::Center,
                // vertically center child text
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .observe(
            |_: Trigger<Pointer<Click>>,
             mut sender: Single<&mut TriggerSender<Ready>, (With<Client>, With<Connected>)>| {
                info!("Client is ready!");
                sender.trigger::<Channel1>(Ready);
            },
        );
}
