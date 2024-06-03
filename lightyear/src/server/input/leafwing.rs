//! Handles client-generated inputs
use std::ops::DerefMut;

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

use crate::inputs::leafwing::input_buffer::{ActionDiffBuffer, InputTarget};
use crate::inputs::leafwing::{InputMessage, LeafwingUserAction};
use crate::prelude::server::MessageEvent;
use crate::prelude::{is_started, MessageRegistry, Mode, TickManager};
use crate::protocol::message::MessageKind;
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{InternalMainSet, ServerMarker};

pub struct LeafwingInputPlugin<A> {
    marker: std::marker::PhantomData<A>,
}

impl<A> Default for LeafwingInputPlugin<A> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    /// Add the ActionDiffBuffers to new entities that have an [`ActionState`]
    AddBuffers,
    /// Receive the latest ActionDiffs from the client
    ReceiveInputs,
    /// Use the ActionDiff received from the client to update the [`ActionState`]
    Update,
}

impl<A: LeafwingUserAction> Plugin for LeafwingInputPlugin<A> {
    fn build(&self, app: &mut App) {
        // RESOURCES
        // app.init_resource::<GlobalActions<A>>();
        // TODO: (global action states) add a resource tracking the action-state of all clients
        // SETS
        app.configure_sets(
            PreUpdate,
            (
                InternalMainSet::<ServerMarker>::Receive,
                InputSystemSet::AddBuffers,
                InputSystemSet::ReceiveInputs,
            )
                .chain()
                .run_if(is_started),
        );
        app.configure_sets(FixedPreUpdate, InputSystemSet::Update.run_if(is_started));
        // SYSTEMS
        app.add_systems(
            PreUpdate,
            (
                // TODO: ideally we have a Flush between add_action_diff_buffer and Tick?
                add_action_diff_buffer::<A>.in_set(InputSystemSet::AddBuffers),
                receive_input_message::<A>.in_set(InputSystemSet::ReceiveInputs),
            ),
        );
        app.add_systems(
            FixedPreUpdate,
            update_action_state::<A>.in_set(InputSystemSet::Update),
        );

        // TODO: register this in Plugin::finish by checking if the client plugin is already registered?
        if app.world.resource::<ServerConfig>().shared.mode != Mode::HostServer {
            // we don't want to add this plugin in HostServer mode because it's already added on the client side
            // Otherwise, we need to add the leafwing server plugin because it ticks Action-States (so just-pressed become pressed)
            app.add_plugins(InputManagerPlugin::<A>::server());
        }
    }
}

/// For each entity that has an action-state, insert an action-state-buffer
/// that will store the value of the action-state for the last few ticks
/// (we use a buffer because the client's inputs might arrive out of order)
fn add_action_diff_buffer<A: LeafwingUserAction>(
    mut commands: Commands,
    action_state: Query<
        Entity,
        (
            Added<ActionState<A>>,
            Without<ActionDiffBuffer<A>>,
            Without<InputMap<A>>,
        ),
    >,
) {
    for entity in action_state.iter() {
        commands
            .entity(entity)
            .insert(ActionDiffBuffer::<A>::default());
    }
}

/// Read the input messages from the server events to update the ActionDiffBuffers
fn receive_input_message<A: LeafwingUserAction>(
    // mut global: Option<ResMut<ActionDiffBuffer<A>>>,
    message_registry: Res<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    mut query: Query<&mut ActionDiffBuffer<A>>,
    mut events: EventWriter<MessageEvent<InputMessage<A>>>,
) {
    let kind = MessageKind::of::<InputMessage<A>>();
    let Some(net) = message_registry.kind_map.net_id(&kind).copied() else {
        error!(
            "Could not find the network id for the message kind: {:?}",
            kind
        );
        return;
    };
    // re-borrow to allow split borrows
    let connection_manager = connection_manager.deref_mut();
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        if let Some(message_list) = connection.received_leafwing_input_messages.remove(&net) {
            for (message_bytes, target, channel_kind) in message_list {
                let mut reader = connection.reader_pool.start_read(&message_bytes);
                match message_registry.deserialize::<InputMessage<A>>(
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok(message) => {
                        debug!(?client_id, action = ?A::short_type_path(), ?message.end_tick, ?message.diffs, "received input message");
                        for (target, diffs) in &message.diffs {
                            match target {
                                // - for pre-predicted entities, we already did the mapping on server side upon receiving the message
                                // (which is possible because the server received the entity)
                                // - for non-pre predicted entities, the mapping was already done on client side
                                // (client converted from their local entity to the remote server entity)
                                InputTarget::Entity(entity)
                                | InputTarget::PrePredictedEntity(entity) => {
                                    debug!("received input for entity: {:?}", entity);
                                    if let Ok(mut buffer) = query.get_mut(*entity) {
                                        debug!(?entity, ?diffs, end_tick = ?message.end_tick, "update action diff buffer for PREPREDICTED using input message");
                                        buffer.update_from_message(message.end_tick, diffs);
                                    } else {
                                        // TODO: maybe if the entity is pre-predicted, apply map-entities, so we can handle pre-predicted inputs
                                        debug!(?entity, ?diffs, end_tick = ?message.end_tick, "received input message for unrecognized entity");
                                    }
                                }
                                InputTarget::Global => {
                                    // TODO: handle global diffs for each client! How? create one entity per client?
                                    //  or have a resource containing the global ActionState for each client?
                                    // if let Some(ref mut buffer) = global {
                                    //     buffer.update_from_message(message.end_tick, std::mem::take(&mut message.global_diffs))
                                    // }
                                }
                            }
                        }

                        // TODO: rebroadcast is never used right now because
                        //  - it's hard to specify on the client who we want to rebroadcast to
                        //  - we shouldn't rebroadcast immediately, instead we want to let the server inspect the input
                        //    to verify that there's no cheating

                        // rebroadcast
                        if target != NetworkTarget::None {
                            if let Ok(message_bytes) =
                                message_registry.serialize(&message, &mut connection_manager.writer)
                            {
                                connection.messages_to_rebroadcast.push((
                                    message_bytes,
                                    target,
                                    channel_kind,
                                ));
                            }
                        }
                        events.send(MessageEvent::new(message, *client_id));
                    }
                    Err(e) => {
                        error!(?e, "could not deserialize leafwing input message");
                    }
                }
                connection.reader_pool.attach(reader);
            }
        }
    }
}

/// Read the ActionDiff for the current tick from the buffer, and use them to update the ActionState
fn update_action_state<A: LeafwingUserAction>(
    tick_manager: Res<TickManager>,
    // global_input_buffer: Res<InputBuffer<A>>,
    // global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<(Entity, &mut ActionState<A>, &mut ActionDiffBuffer<A>)>,
) {
    let tick = tick_manager.tick();

    for (entity, mut action_state, mut action_diff_buffer) in action_state_query.iter_mut() {
        // the state on the server is only updated from client inputs!
        trace!(
            ?tick,
            ?entity,
            ?action_diff_buffer,
            "action state: {:?}. Latest action diff buffer tick: {:?}",
            &action_state.get_pressed(),
            action_diff_buffer.end_tick(),
        );
        action_diff_buffer.pop(tick).into_iter().for_each(|diff| {
            debug!(
                ?tick,
                ?entity,
                "update action state using action diff: {:?}",
                &diff
            );
            diff.apply(action_state.deref_mut());
        });
        debug!(?tick, ?entity, pressed = ?action_state.get_pressed(), "action state after update");
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::InputPlugin;
    use bevy::utils::Duration;
    use leafwing_input_manager::prelude::ActionState;

    use crate::inputs::leafwing::input_buffer::{ActionDiff, InputBuffer};
    use crate::prelude::client;
    use crate::prelude::client::{
        InterpolationConfig, LeafwingInputConfig, PredictionConfig, SyncConfig,
    };
    use crate::prelude::server::*;
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    use super::*;

    #[test]
    fn test_leafwing_inputs() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(0),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default();
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = BevyStepper::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            link_conditioner,
            frame_duration,
        );
        #[cfg(feature = "leafwing")]
        {
            // NOTE: the test doesn't work with send_diffs_only = True; maybe because leafwing's
            //  tick-action uses Time<Real>?
            let config = LeafwingInputConfig {
                send_diffs_only: false,
                ..default()
            };
            stepper
                .client_app
                .add_plugins(crate::prelude::LeafwingInputPlugin::<LeafwingInput1> {
                    config: config.clone(),
                });
            stepper
                .client_app
                .add_plugins(crate::prelude::LeafwingInputPlugin::<LeafwingInput2>::default());
            stepper
                .server_app
                .add_plugins(crate::prelude::LeafwingInputPlugin::<LeafwingInput1> {
                    config: config.clone(),
                });
            stepper
                .server_app
                .add_plugins(crate::prelude::LeafwingInputPlugin::<LeafwingInput2>::default());
        }
        stepper.client_app.add_plugins(InputPlugin);
        stepper.init();

        // create an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((
                ActionState::<LeafwingInput1>::default(),
                Replicate::default(),
            ))
            .id();
        // we need to step twice because we run client before server
        stepper.frame_step();
        stepper.frame_step();

        // check that the server entity got a ActionDiffBuffer added to it
        assert!(stepper
            .server_app
            .world
            .entity(server_entity)
            .get::<ActionDiffBuffer<LeafwingInput1>>()
            .is_some());

        // check that the entity is replicated
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .is_some());
        // add an InputMap to the client entity
        stepper
            .client_app
            .world
            .entity_mut(client_entity)
            .insert(InputMap::<LeafwingInput1>::new([(
                LeafwingInput1::Jump,
                KeyCode::KeyA,
            )]));
        stepper.frame_step();
        // check that the client entity got an InputBuffer added to it
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .is_some());

        // update the ActionState on the client by pressing on the button once
        stepper
            .client_app
            .world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        debug!("before press");
        stepper.frame_step();
        // client tick when we send the Jump action
        let client_tick = stepper.client_tick();
        // we should have sent an InputMessage from client to server
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<ActionDiffBuffer<LeafwingInput1>>()
                .unwrap()
                .get(client_tick),
            vec![ActionDiff::Pressed {
                action: LeafwingInput1::Jump
            }]
        );
        stepper
            .client_app
            .world
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        // TODO: how come I need to frame_step() twice to see the release action?
        debug!("before release");
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world
                .entity(server_entity)
                .get::<ActionDiffBuffer<LeafwingInput1>>()
                .unwrap()
                .get(client_tick + 1),
            vec![ActionDiff::Released {
                action: LeafwingInput1::Jump
            }]
        );
    }
}
