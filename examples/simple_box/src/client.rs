//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to predicted entities. In client/server mode this is the locally controlled
//!   player; in P2P mode every peer predicts the complete deterministic roster.

use crate::automation::{self, AutomationClientPlugin};
use crate::protocol::*;
use crate::shared;
use bevy::prelude::*;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::client::input::*;
use lightyear::prelude::client::{InputDelayConfig, InputTimelineConfig};
use lightyear::prelude::input::native::*;
use lightyear::prelude::*;
#[cfg(feature = "p2p")]
use lightyear_examples_common::p2p::P2PSettings;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_systems(Startup, configure_input_delay);
        app.add_systems(
            FixedPreUpdate,
            // Inputs have to be buffered in the WriteClientInputs set
            buffer_input.in_set(InputSystems::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, player_movement);

        app.add_systems(Update, receive_message1);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_controlled_spawn);
        app.add_observer(handle_interpolated_spawn);
    }
}

fn configure_input_delay(mut commands: Commands) {
    commands.insert_resource(
        InputTimelineConfig::default().with_input_delay(InputDelayConfig::no_input_delay()),
    );
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
///
/// I would also advise to use the `leafwing` feature to use the `LeafwingInputPlugin` instead of the
/// `InputPlugin`, which contains more features.
fn buffer_input(
    timeline: Res<LocalTimeline>,
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    automation: Option<Res<automation::client::AutomationSettings>>,
    keypress: Option<Res<ButtonInput<KeyCode>>>,
) {
    if let Ok(mut action_state) = query.single_mut() {
        let current_tick = timeline.tick();
        let mut direction =
            automation::client::direction_override(automation, current_tick).unwrap_or_default();

        if direction.is_none() {
            if let Some(keypress) = keypress {
                if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
                    direction.up = true;
                }
                if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
                    direction.down = true;
                }
                if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
                    direction.left = true;
                }
                if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
                    direction.right = true;
                }
            }
        }
        // we always set the value. Setting it to None means that the input was missing, it's not the same
        // as saying that the input was 'no keys pressed'
        action_state.0 = Inputs::Direction(direction);
        trace!(
            target: "lightyear_debug::simple_box",
            kind = "simple_box_client_input",
            schedule = "FixedPreUpdate",
            sample_point = "FixedPreUpdate",
            local_tick = current_tick.0,
            input = ?action_state.0,
            "selected simple_box client input"
        );
    }
}

/// Apply movement to every entity simulated by this client.
///
/// Conventional clients only have a [`Predicted`] copy of their locally controlled player. P2P
/// peers instead give every roster member [`DeterministicPredicted`]: the local player uses captured
/// input, while remote players repeat their latest known input and trigger rollback when corrected
/// input arrives.
fn player_movement(
    _input_timeline: SyncedInputTimeline,
    timeline: Res<LocalTimeline>,
    #[cfg(feature = "p2p")] p2p: Option<Res<P2PSettings>>,
    mut position_query: Query<
        (&mut PlayerPosition, &ActionState<Inputs>),
        Or<(With<Predicted>, With<DeterministicPredicted>)>,
    >,
) {
    #[cfg(feature = "p2p")]
    if p2p.is_some() && timeline.tick().0 < lightyear_examples_common::p2p::GAMEPLAY_START_TICK {
        return;
    }
    for (position, input) in position_query.iter_mut() {
        // trace!(?tick, ?position, ?input, "client");
        // Pass Mut<PlayerPosition> directly so change detection only fires when movement changes it.
        shared::shared_movement_behaviour(position, input);
    }
}

/// System to receive messages on the client
pub(crate) fn receive_message1(
    metadata: Res<NetworkingMetadata>,
    mut receivers: Query<&mut MessageReceiver<Message1>>,
) {
    let link = match &metadata.mode {
        NetworkTopology::Client(link) => *link,
        NetworkTopology::HostClient { client, .. } => *client,
        _ => return,
    };
    let Ok(mut receiver) = receivers.get_mut(link) else {
        return;
    };
    for message in receiver.receive() {
        info!("Received message: {:?}", message);
    }
}

/// Lower the saturation on predicted entities so they are visually distinct.
///
/// Note that this will be triggered multiple times: for the locally-controlled entity,
/// but also for the remote-controlled entities that are spawned with [`Interpolated`].
/// The `With<Predicted>` filter ensures we only add the `InputMarker` once.
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut predicted: Query<&mut PlayerColor, With<Predicted>>,
) {
    let entity = trigger.entity;
    if let Ok(mut color) = predicted.get_mut(entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

fn handle_controlled_spawn(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    players: Query<&PlayerId, Without<InputMarker<Inputs>>>,
) {
    let entity = trigger.entity;
    let Ok(player_id) = players.get(entity) else {
        return;
    };
    info!("Adding InputMarker to controlled player {entity:?} {player_id:?}");
    commands
        .entity(entity)
        .insert(InputMarker::<Inputs>::default());
}

/// Lower the saturation on interpolated entities so they are visually distinct.
pub(crate) fn handle_interpolated_spawn(
    trigger: On<Add, Interpolated>,
    mut interpolated: Query<&mut PlayerColor>,
) {
    if let Ok(mut color) = interpolated.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}
