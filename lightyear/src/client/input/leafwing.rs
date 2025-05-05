//! Module to handle inputs that are defined using the `leafwing_input_manager` crate
//!
//! ### Adding leafwing inputs
//!
//! You first need to create Inputs that are defined using the [`leafwing_input_manager`](https://github.com/Leafwing-Studios/leafwing-input-manager) crate.
//! (see the documentation of the crate for more information)
//! In particular your inputs should implement the [`Actionlike`] trait.
//!
//! ```rust
//! use bevy::prelude::*;
//! use lightyear::prelude::*;
//! use lightyear::prelude::client::*;
//! use leafwing_input_manager::Actionlike;
//! use serde::{Deserialize, Serialize};
//! #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
//! pub enum PlayerActions {
//!     Up,
//!     Down,
//!     Left,
//!     Right,
//! }
//!
//! let mut app = App::new();
//! app.add_plugins(LeafwingInputPlugin::<PlayerActions>::default());
//! ```
//!
//! ### Usage
//!
//! The networking of inputs is completely handled for you. You just need to add the `LeafwingInputPlugin` to your app.
//! Make sure that all your systems that depend on user inputs are added to the [`FixedUpdate`] [`Schedule`].
//!
//! Currently, global inputs (that are stored in a [`Resource`] instead of being attached to a specific [`Entity`] are not supported)
use core::fmt::Debug;

use bevy::prelude::*;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use tracing::{error, trace};

use crate::channel::builder::InputChannel;
use crate::client::components::Confirmed;
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::input::{BaseInputPlugin, InputSystemSet};
use crate::client::prediction::plugin::is_in_rollback;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::Predicted;
use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::inputs::leafwing::input_message::InputTarget;
use crate::inputs::leafwing::LeafwingUserAction;
use crate::prelude::{
    is_host_server, ChannelKind, ChannelRegistry, ClientReceiveMessage, InputMessage,
    MessageRegistry, TickManager, TimeManager,
};
use crate::shared::input::InputConfig;
use crate::shared::replication::components::PrePredicted;
use crate::shared::tick_manager::TickEvent;

// TODO: is this actually necessary? The sync happens in PostUpdate,
//  so maybe it's ok if the InputMessages contain the pre-sync tick! (since those inputs happened
//  before the sync). If it's not needed, send the messages directly in FixedPostUpdate!
//  Actually maybe it is, because the send-tick on the server will be updated.
/// Buffer that will store the InputMessages we want to write this frame.
///
/// We need this because:
/// - we write the InputMessages during FixedPostUpdate
/// - we apply the TickUpdateEvents (from doing sync) during PostUpdate, which might affect the ticks from the InputMessages.
///   During this phase, we want to update the tick of the InputMessages that we wrote during FixedPostUpdate.
#[derive(Debug, Resource)]
struct MessageBuffer<A: LeafwingUserAction>(Vec<InputMessage<A>>);

impl<A: LeafwingUserAction> Default for MessageBuffer<A> {
    fn default() -> Self {
        Self(Vec::default())
    }
}

/// Adds a plugin to handle inputs using the LeafwingInputManager
pub struct LeafwingInputPlugin<A> {
    config: InputConfig<A>,
}

impl<A> LeafwingInputPlugin<A> {
    pub fn new(config: InputConfig<A>) -> Self {
        Self { config }
    }
}

impl<A> Default for LeafwingInputPlugin<A> {
    fn default() -> Self {
        Self::new(InputConfig::default())
    }
}

impl<A: LeafwingUserAction> Plugin for LeafwingInputPlugin<A>
// FLOW WITH INPUT DELAY
// - pre-update: run leafwing to update the current ActionState, which is the action-state for tick T + delay
// - fixed-pre-update:
//   - we write the current action-diffs to the buffer for tick T + d (for sending messages to server)
//   - we write the current action-state to the buffer for tick T + d (for rollbacks)
//   - get the action-state for tick T from the buffer
// - fixed-update:
//   - we use the action-state for tick T (that we got from the buffer)
// - fixed-post-update:
//   - we fetch the action-state for tick T + d from the buffer and set it on the ActionState
//     (so that it's ready for the next frame's PreUpdate, or for the next FixedPreUpdate)
// - update:
//   - the ActionState is not usable in Update, because we have the ActionState for tick T + d
// TODO: will need to generate diffs in FixedPreUpdate schedule once it's fixed in leafwing
{
    fn build(&self, app: &mut App) {
        // PLUGINS
        app.add_plugins(InputManagerPlugin::<A>::default());
        app.add_plugins(BaseInputPlugin::<ActionState<A>, InputMap<A>>::default());

        // in host-server mode, we don't need to handle inputs in any way, because the player's entity
        // is spawned with `InputBuffer` and the client is in the same timeline as the server
        let should_run = not(is_host_server);

        // RESOURCES
        app.insert_resource(self.config);
        app.init_resource::<MessageBuffer<A>>();

        // SETS
        app.configure_sets(
            FixedPostUpdate,
            InputSystemSet::RestoreInputs.before(InputManagerSystem::Tick),
        );

        // SYSTEMS
        if self.config.rebroadcast_inputs {
            app.add_systems(
                RunFixedMainLoop,
                receive_remote_player_input_messages::<A>
                    .in_set(InputSystemSet::ReceiveInputMessages),
            );
        }

        app.add_systems(
            FixedPostUpdate,
            prepare_input_message::<A>
                .in_set(InputSystemSet::PrepareInputMessage)
                // no need to prepare messages to send if in rollback
                .run_if(not(is_in_rollback)),
        );
        app.add_systems(
            PostUpdate,
            send_input_messages::<A>.in_set(InputSystemSet::SendInputMessage),
        );
        // if the client tick is updated because of a desync, update the ticks in the input buffers
        app.add_observer(receive_tick_events::<A>);
    }
}

/// Send a message to the server containing the ActionDiffs for the last few ticks
fn prepare_input_message<A: LeafwingUserAction>(
    connection: Res<ConnectionManager>,
    mut message_buffer: ResMut<MessageBuffer<A>>,
    channel_registry: Res<ChannelRegistry>,
    config: Res<ClientConfig>,
    input_config: Res<InputConfig<A>>,
    tick_manager: Res<TickManager>,
    input_buffer_query: Query<
        (
            Entity,
            &InputBuffer<A>,
            Option<&Predicted>,
            Option<&PrePredicted>,
        ),
        With<InputMap<A>>,
    >,
) -> Result {
    // we send a message from the latest tick that we have available, which is the delayed tick
    let input_delay_ticks = connection.input_delay_ticks() as i16;
    let tick = tick_manager.tick() + input_delay_ticks;
    // TODO: the number of messages should be in SharedConfig
    trace!(delayed_tick = ?tick, current_tick = ?tick_manager.tick(), "prepare_input_message");
    // TODO: instead of redundancy, send ticks up to the latest yet ACK-ed input tick
    //  this means we would also want to track packet->message acks for unreliable channels as well, so we can notify
    //  this system what the latest acked input tick is?
    let input_send_interval = channel_registry
        .get_builder_from_kind(&ChannelKind::of::<InputChannel>())
        .unwrap()
        .settings
        .send_frequency;
    // we send redundant inputs, so that if a packet is lost, we can still recover
    // A redundancy of 2 means that we can recover from 1 lost packet
    let mut num_tick: u16 =
        ((input_send_interval.as_nanos() / config.shared.tick.tick_duration.as_nanos()) + 1)
            .try_into()?;
    num_tick *= input_config.packet_redundancy;
    let mut message = InputMessage::<A>::new(tick);
    for (entity, input_buffer, predicted, pre_predicted) in input_buffer_query.iter() {
        trace!(
            ?tick,
            ?entity,
            "Preparing input message with buffer: {:?}",
            input_buffer
        );

        // Make sure that server can read the inputs correctly
        // TODO: currently we are not sending inputs for pre-predicted entities until we receive the confirmation from the server
        //  could we find a way to do it?
        //  maybe if it's pre-predicted, we send the original entity (pre-predicted), and the server will apply the conversion
        //   on their end?
        if pre_predicted.is_some() {
            // wait until the client receives the PrePredicted entity confirmation to send inputs
            // otherwise we get failed entity_map logs
            // TODO: the problem is that we wait until we have received the server answer. Ideally we would like
            //  to wait until the server has received the PrePredicted entity
            if predicted.is_none() {
                continue;
            }
            trace!(
                ?tick,
                "sending inputs for pre-predicted entity! Local client entity: {:?}",
                entity
            );
            // TODO: not sure if this whole pre-predicted inputs thing is worth it, because the server won't be able to
            //  to receive the inputs until it receives the pre-predicted spawn message.
            //  so all the inputs sent between pre-predicted spawn and server-receives-pre-predicted will be lost

            // TODO: I feel like pre-predicted inputs work well only for global-inputs, because then the server can know
            //  for which client the inputs were!

            // 0. the entity is pre-predicted, no need to convert the entity (the mapping will be done on the server, when
            // receiving the message. It's possible because the server received the PrePredicted entity before)
            message.add_inputs(
                num_tick,
                InputTarget::PrePredictedEntity(entity),
                input_buffer,
            );
        } else {
            // 1. if the entity is confirmed, we need to convert the entity to the server's entity
            // 2. if the entity is predicted, we need to first convert the entity to confirmed, and then from confirmed to remote
            if let Some(confirmed) = predicted.map_or(Some(entity), |p| p.confirmed_entity) {
                if let Some(server_entity) = connection
                    .replication_receiver
                    .remote_entity_map
                    .get_remote(confirmed)
                {
                    trace!("sending input for server entity: {:?}. local entity: {:?}, confirmed: {:?}", server_entity, entity, confirmed);
                    // println!(
                    //     "preparing input message using input_buffer: {}",
                    //     input_buffer
                    // );
                    message.add_inputs(num_tick, InputTarget::Entity(server_entity), input_buffer);
                }
            } else {
                // TODO: entity is not predicted or not confirmed? also need to do the conversion, no?
                trace!("not sending inputs because couldnt find server entity");
            }
        }
    }

    trace!(
        ?tick,
        ?num_tick,
        "sending input message for {:?}: {}",
        A::short_type_path(),
        message
    );
    message_buffer.0.push(message);

    Ok(())
    // NOTE: keep the older input values in the InputBuffer! because they might be needed when we rollback for client prediction
}

/// Drain the messages from the buffer and send them to the server
fn send_input_messages<A: LeafwingUserAction>(
    mut connection: ResMut<ConnectionManager>,
    input_config: Res<InputConfig<A>>,
    mut message_buffer: ResMut<MessageBuffer<A>>,
    time_manager: Res<TimeManager>,
    tick_manager: Res<TickManager>,
) -> Result {
    trace!(
        "Number of input messages to send: {:?}",
        message_buffer.0.len()
    );
    for mut message in message_buffer.0.drain(..) {
        // if lag compensation is enabled, we send the current delay to the server
        // (this runs here because the delay is only correct after the SyncSet has run)
        // TODO: or should we actually use the interpolation_delay BEFORE SyncSet
        //  because the user is reacting to stuff from the previous frame?
        if input_config.lag_compensation {
            message.interpolation_delay = Some(
                connection
                    .sync_manager
                    .interpolation_delay(tick_manager.as_ref(), time_manager.as_ref()),
            );
        }
        connection
            .send_message::<InputChannel, InputMessage<A>>(&message)?;
    }
    Ok(())
}

/// Read the InputMessages of other clients from the server to update their InputBuffer and ActionState.
/// This is useful if we want to do client-prediction for remote players.
///
/// If the InputBuffer/ActionState is missing, we will add it.
///
/// We will apply the diffs on the Predicted entity.
fn receive_remote_player_input_messages<A: LeafwingUserAction>(
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    mut received_inputs: ResMut<Events<ClientReceiveMessage<InputMessage<A>>>>,
    connection: Res<ConnectionManager>,
    prediction_manager: Res<PredictionManager>,
    message_registry: Res<MessageRegistry>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    confirmed_query: Query<&Confirmed, Without<InputMap<A>>>,
    mut predicted_query: Query<
        Option<&mut InputBuffer<A>>,
        (Without<InputMap<A>>, With<Predicted>),
    >,
) {
    let tick = tick_manager.tick();
    received_inputs.drain().for_each(|event| {
        let message = event.message;
        trace!(?tick, action = ?A::short_type_path(), ?message.end_tick, %message, "received remote input message");
        for target_data in &message.diffs {
            // - the input target has already been set to the server entity in the InputMessage
            // - it has been mapped to a client-entity on the client during deserialization
            //   ONLY if it's PrePredicted (look at the MapEntities implementation)
            let entity = match target_data.target {
                InputTarget::Entity(entity) => {
                    // TODO: find a better way!
                    // if InputTarget = Entity, we still need to do the mapping
                    connection
                        .replication_receiver
                        .remote_entity_map
                        .get_local(entity)
                }
                InputTarget::PrePredictedEntity(entity) => Some(entity),
            };
            if let Some(entity) = entity {
                trace!(
                    "received input message for entity: {:?}. Applying to diff buffer.",
                    entity
                );
                if let Ok(confirmed) = confirmed_query.get(entity) {
                    if let Some(predicted) = confirmed.predicted {
                        if let Ok(input_buffer) = predicted_query.get_mut(predicted) {
                            trace!(?entity, ?target_data.diffs, end_tick = ?message.end_tick, "update action diff buffer for remote player PREDICTED using input message");
                            if let Some(mut input_buffer) = input_buffer {
                                input_buffer.update_from_diffs(
                                    message.end_tick,
                                    &target_data.start_state,
                                    &target_data.diffs,
                                );
                                trace!("input buffer after update: {:?}", input_buffer);
                                #[cfg(feature = "metrics")]
                                {
                                    let margin = input_buffer.end_tick().unwrap() - tick;
                                    metrics::gauge!(format!(
                                                    "inputs::{}::remote_player::{}::buffer_margin",
                                                    core::any::type_name::<A>(),
                                                    entity
                                                ))
                                        .set(margin as f64);
                                    metrics::gauge!(format!(
                                                    "inputs::{}::remote_player::{}::buffer_size",
                                                    core::any::type_name::<A>(),
                                                    entity
                                                ))
                                        .set(input_buffer.len() as f64);
                                }
                            } else {
                                debug!(?entity, "Inserting input buffer for remote player!");
                                // add the ActionState or InputBuffer if they are missing
                                let mut input_buffer = InputBuffer::<A>::default();
                                input_buffer.update_from_diffs(
                                    message.end_tick,
                                    &target_data.start_state,
                                    &target_data.diffs,
                                );
                                // if the remote_player's predicted entity doesn't have the InputBuffer, we need to insert them
                                commands.entity(predicted).insert((
                                    input_buffer,
                                    ActionState::<A>::default(),
                                ));
                            };
                        }
                    }
                } else {
                    error!(?entity, ?target_data.diffs, end_tick = ?message.end_tick, "received input message for unrecognized entity");
                }
            } else {
                error!("received remote player input message for unrecognized entity");
            }
        }
    });
}

/// In case the client tick changes suddenly, we also update the InputBuffer accordingly
fn receive_tick_events<A: LeafwingUserAction>(
    trigger: Trigger<TickEvent>,
    mut message_buffer: ResMut<MessageBuffer<A>>,
    mut input_buffer_query: Query<&mut InputBuffer<A>>,
) {
    match *trigger.event() {
        TickEvent::TickSnap { old_tick, new_tick } => {
            for mut input_buffer in input_buffer_query.iter_mut() {
                if let Some(start_tick) = input_buffer.start_tick {
                    input_buffer.start_tick = Some(start_tick + (new_tick - old_tick));
                    debug!(
                        "Receive tick snap event {:?}. Updating input buffer start_tick to {:?}!",
                        trigger.event(),
                        input_buffer.start_tick
                    );
                }
            }
            for message in message_buffer.0.iter_mut() {
                message.end_tick = message.end_tick + (new_tick - old_tick);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;
    use leafwing_input_manager::action_state::ActionState;
    use leafwing_input_manager::input_map::InputMap;

    use crate::prelude::client::{InterpolationDelay, PredictionConfig};
    use crate::prelude::server::{Replicate, SyncTarget};
    use crate::prelude::{client, NetworkTarget, ServerReceiveMessage, ServerSendMessage, SharedConfig, TickConfig};
    use crate::tests::multi_stepper::MultiBevyStepper;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;

    fn build_stepper_with_input_delay(delay_ticks: u16) -> BevyStepper {
        let frame_duration = Duration::from_millis(10);
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..default()
        };
        let client_config = ClientConfig {
            prediction: PredictionConfig {
                minimum_input_delay_ticks: delay_ticks,
                maximum_input_delay_before_prediction: delay_ticks,
                maximum_predicted_ticks: 30,
                ..default()
            },
            ..default()
        };
        let mut stepper = BevyStepper::new(shared_config, client_config, frame_duration);
        stepper.build();
        stepper.init();
        stepper
    }

    fn setup(stepper: &mut BevyStepper) -> (Entity, Entity) {
        // create an entity on server
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((
                ActionState::<LeafwingInput1>::default(),
                Replicate::default(),
            ))
            .id();
        // we need to step twice because we run client before server
        stepper.frame_step();
        stepper.frame_step();

        // check that the server entity got a InputBuffer added to it
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .is_some());

        // check that the entity is replicated, including the ActionState component
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        stepper
            .client_app
            .world_mut()
            .entity_mut(client_entity)
            .insert(InputMap::<LeafwingInput1>::new([(
                LeafwingInput1::Jump,
                KeyCode::KeyA,
            )]));
        stepper.frame_step();
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .is_some());
        (server_entity, client_entity)
    }

    /// Check that ActionStates are stored correctly in the InputBuffer
    // TODO: for the test to work correctly, I need to inspect the state during FixedUpdate schedule!
    //  otherwise the test gives me the input values outside of FixedUpdate, which is not what I want...
    //  disable the test for now until we figure it out
    #[test]
    fn test_buffer_inputs_no_delay() {
        let mut stepper = BevyStepper::default();
        let (server_entity, client_entity) = setup(&mut stepper);

        // press on a key
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        stepper.frame_step();
        let client_tick = stepper.client_tick();
        let input_buffer = stepper
            .client_app
            .world_mut()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap();
        // check that the action state got buffered
        // (we cannot use JustPressed because we start by ticking the ActionState)
        assert_eq!(
            input_buffer.get(client_tick).unwrap().get_just_pressed(),
            &[LeafwingInput1::Jump]
        );

        // test with another frame
        stepper.frame_step();
        let input_buffer = stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap();
        assert_eq!(
            input_buffer.get(client_tick + 1).unwrap().get_pressed(),
            &[LeafwingInput1::Jump]
        );

        // try releasing the key
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        stepper.frame_step();
        let input_buffer = stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap();
        assert_eq!(
            input_buffer
                .get(client_tick + 2)
                .unwrap()
                .get_just_released(),
            &[LeafwingInput1::Jump]
        );
        assert!(input_buffer
            .get(client_tick + 2)
            .unwrap()
            .get_just_pressed()
            .is_empty());
    }

    /// Check that ActionStates are stored correctly in the InputBuffer
    #[test]
    fn test_buffer_inputs_with_delay() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let mut stepper = build_stepper_with_input_delay(1);
        let (server_entity, client_entity) = setup(&mut stepper);

        // press on a key
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        stepper.frame_step();
        let client_tick = stepper.client_tick();

        // check that the action state got buffered without any press (because the input is delayed)
        // (we cannot use JustPressed because we start by ticking the ActionState)
        // (i.e. the InputBuffer is empty for the current tick, and has the button press only with 1 tick of delay)
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick)
            .unwrap()
            .get_pressed()
            .is_empty());
        // if we check the next tick (delay of 1), we can see that the InputBuffer contains the ActionState with a press
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick + 1)
            .unwrap()
            .pressed(&LeafwingInput1::Jump));

        // outside of the FixedUpdate schedule, the fixed_update_state of ActionState should be the delayed action
        // (which we restored)
        //
        // It has been ticked by LWIM so now it's only pressed
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .button_data(&LeafwingInput1::Jump)
            .unwrap()
            .fixed_update_state
            .pressed());

        // release the key
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        // TODO: ideally we would check that the value of the ActionState inside FixedUpdate is correct
        // step another frame, this time we get the buffered input from earlier
        stepper.frame_step();
        let input_buffer = stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap();
        assert_eq!(
            input_buffer.get(client_tick + 1).unwrap().get_pressed(),
            &[LeafwingInput1::Jump]
        );
        // the fixed_update_state ActionState outside of FixedUpdate is the delayed one
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .button_data(&LeafwingInput1::Jump)
            .unwrap()
            .fixed_update_state
            .released());

        stepper.frame_step();

        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick + 2)
            .unwrap()
            .just_released(&LeafwingInput1::Jump));
    }

    /// Check that the interpolation delay is sent correctly,
    /// and that the server inserts an Interpolation Delay component
    #[test]
    fn test_send_inputs_with_lag_compensation() {
        let mut stepper = BevyStepper::default();
        stepper
            .client_app
            .world_mut()
            .resource_mut::<InputConfig<LeafwingInput1>>()
            .lag_compensation = true;
        let (server_entity, client_entity) = setup(&mut stepper);

        // The InterpolationDelay component should have been added on the server
        // on the entity corresponding to the client
        let delay = stepper
            .server_app
            .world_mut()
            .query::<&InterpolationDelay>()
            .get_single(stepper.server_app.world())
            .unwrap();
        assert_ne!(delay.delay_ms, 0);
    }

    pub(crate) fn replicate_inputs(
        mut receive_inputs: ResMut<Events<ServerReceiveMessage<InputMessage<LeafwingInput1>>>>,
        mut send_inputs: EventWriter<ServerSendMessage<InputMessage<LeafwingInput1>>>,
    ) {
        // rebroadcast the input to other clients
        // we are calling drain() here so make sure that this system runs after the `ReceiveInputs` set,
        // so that the server had the time to process the inputs
        send_inputs.send_batch(receive_inputs.drain().map(|ev| {
            ServerSendMessage::new_with_target::<InputChannel>(
                ev.message,
                NetworkTarget::AllExceptSingle(ev.from),
            )
        }));
    }
    #[test]
    fn test_receive_inputs_other_clients() {
        let mut stepper = MultiBevyStepper::default();
        // server propagate inputs to other clients
        stepper.server_app.add_systems(
            PreUpdate,
            replicate_inputs.after(crate::server::input::leafwing::InputSystemSet::ReceiveInputs),
        );
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((
                ActionState::<LeafwingInput1>::default(),
                Replicate {
                    sync: SyncTarget {
                        prediction: NetworkTarget::All,
                        interpolation: NetworkTarget::None,
                    },
                    ..default()
                },
            ))
            .id();

        stepper.frame_step();
        stepper.frame_step();

        let client_entity_2 = stepper
            .client_app_2
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        let client_entity_1 = stepper
            .client_app_1
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        stepper
            .client_app_2
            .world_mut()
            .entity_mut(client_entity_2)
            .insert(InputMap::<LeafwingInput1>::new([(
                LeafwingInput1::Jump,
                KeyCode::KeyA,
            )]));
        stepper.frame_step();
        stepper.frame_step();

        // client 1 should have received the InputMessage from client 2 which was broadcasted by the client
        assert!(stepper
            .client_app_1
            .world()
            .get::<ActionState<LeafwingInput1>>(client_entity_1)
            .is_some());
        assert!(stepper
            .client_app_1
            .world()
            .get::<InputBuffer<LeafwingInput1>>(client_entity_1)
            .unwrap()
            .end_tick()
            .is_some());
    }
}
