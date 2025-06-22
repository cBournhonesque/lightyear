use crate::protocol::*;
use bevy::prelude::*;
use lightyear::prelude::Confirmed;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        #[cfg(feature = "client")]
        app.add_systems(Startup, rollback_button);
        app.add_systems(Update, draw_boxes);
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// System that draws the boxes of the player positions.
/// The components should be replicated from the server to the client
pub(crate) fn draw_boxes(
    mut gizmos: Gizmos,
    players: Query<(&PlayerPosition, &PlayerColor), Without<Confirmed>>,
) {
    for (position, color) in &players {
        gizmos.rect_2d(
            Isometry2d::from_translation(position.0),
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}

#[cfg(feature = "client")]
pub(crate) fn rollback_button(mut commands: Commands) {
    use lightyear::prelude::{LocalTimeline, NetworkTimeline, PredictionManager, Rollback};
    commands
        .spawn((
            Text("Rollback".to_string()),
            TextColor(Color::srgb(0.9, 0.9, 0.9)),
            TextFont::from_font_size(20.0),
            Node {
                width: Val::Px(150.0),
                height: Val::Px(65.0),
                border: UiRect::all(Val::Px(5.0)),
                left: Val::Percent(45.0),
                // horizontally center child text
                justify_content: JustifyContent::Center,
                // vertically center child text
                align_items: AlignItems::Center,
                ..default()
            },
            Button,
        ))
        .observe(
            |_: Trigger<Pointer<Click>>,
             mut commands: Commands,
             client: Single<(Entity, &LocalTimeline, &mut PredictionManager)>,
             mut confirmed: Query<&mut Confirmed>| {
                let (client, local_timeline, prediction_manager) = client.into_inner();

                // rollback the client to 5 ticks before the current tick
                let tick = local_timeline.tick();
                let rollback_tick = tick - 5;
                info!("Manual rollback to tick {rollback_tick:?}. Current tick: {tick:?}");
                commands.entity(client).insert(Rollback);
                prediction_manager.set_rollback_tick(rollback_tick);
                confirmed.iter_mut().for_each(|mut confirmed| {
                    confirmed.tick = rollback_tick;
                });
            },
        );
}
