//! Handles client-generated inputs
use std::ops::DerefMut;

use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::inputs::leafwing::input_message::InputTarget;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

use crate::inputs::leafwing::LeafwingUserAction;
use crate::prelude::server::ReceiveMessage;
use crate::prelude::{
    server::is_started, InputMessage, MessageRegistry, Mode, ServerReceiveMessage, TickManager,
};
use crate::protocol::message::MessageKind;
use crate::serialize::reader::Reader;
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
                // TODO: can disable this in host-server mode!
                receive_input_message::<A>.in_set(InputSystemSet::ReceiveInputs),
            ),
        );
        app.add_systems(
            FixedPreUpdate,
            update_action_state::<A>.in_set(InputSystemSet::Update),
        );

        // TODO: register this in Plugin::finish by checking if the client plugin is already registered?
        if app.world().resource::<ServerConfig>().shared.mode != Mode::HostServer {
            // we don't want to add this plugin in HostServer mode because it's already added on the client side
            // Otherwise, we need to add the leafwing server plugin because it ticks Action-States (so just-pressed become pressed)
            app.add_plugins(InputManagerPlugin::<A>::server());
        }
    }
}

/// For each entity that has an action-state, insert an InputBuffer, to store
/// the values of the ActionState for the ticks of the message
fn add_action_diff_buffer<A: LeafwingUserAction>(
    mut commands: Commands,
    action_state: Query<Entity, (Added<ActionState<A>>, Without<InputMap<A>>)>,
) {
    for entity in action_state.iter() {
        commands.entity(entity).insert(InputBuffer::<A>::default());
    }
}

/// Read the input messages from the server events to update the InputBuffers
fn receive_input_message<A: LeafwingUserAction>(
    message_registry: Res<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    mut query: Query<Option<&mut InputBuffer<A>>>,
    mut commands: Commands,
    mut events: EventWriter<ServerReceiveMessage<InputMessage<A>>>,
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
        if let Some(message_list) = connection.received_leafwing_input_messages.get_mut(&net) {
            message_list.drain(..).for_each(|(message_bytes, target, channel_kind)| {
                let mut reader = Reader::from(message_bytes);
                match message_registry.deserialize::<InputMessage<A>>(
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok(message) => {
                        trace!(?client_id, action = ?A::short_type_path(), ?message.end_tick, ?message.diffs, "received input message");
                        // TODO: or should we try to store in a buffer the interpolation delay for the exact tick
                        //  that the message was intended for?
                        // update the interpolation delay estimate for the client
                        if let Some(interpolation_delay) = message.interpolation_delay {
                            commands
                                .entity(connection.entity)
                                .insert(interpolation_delay);
                        }
                        // TODO: UPDATE THIS
                        for data in &message.diffs {
                            match data.target {
                                // - for pre-predicted entities, we already did the mapping on server side upon receiving the message
                                // (which is possible because the server received the entity)
                                // - for non-pre predicted entities, the mapping was already done on client side
                                // (client converted from their local entity to the remote server entity)
                                InputTarget::Entity(entity)
                                | InputTarget::PrePredictedEntity(entity) => {
                                    // TODO Don't update input buffer if inputs arrived too late?
                                    trace!("received input for entity: {:?}", entity);

                                    if let Ok(buffer) = query.get_mut(entity) {
                                        if let Some(mut buffer) = buffer {
                                            trace!(
                                                ?target,
                                                "Update InputBuffer: {} using InputMessage: {}",
                                                buffer.as_ref(),
                                                message
                                            );
                                            buffer.update_from_message(
                                                message.end_tick,
                                                &data.start_state,
                                                &data.diffs,
                                            );
                                        } else {
                                            debug!("Adding InputBuffer and ActionState which are missing on the entity");
                                            commands.entity(entity).insert((
                                                InputBuffer::<A>::default(),
                                                ActionState::<A>::default(),
                                            ));
                                        }
                                    } else {
                                        debug!(?entity, ?data.diffs, end_tick = ?message.end_tick, "received input message for unrecognized entity");
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
                        //  Instead, add a system that manually rebroadcast inputs

                        // rebroadcast
                        if target != NetworkTarget::None {
                            if let Ok(()) = message_registry.serialize(
                                &message,
                                &mut connection_manager.writer,
                                    &mut connection
                                        .replication_receiver
                                        .remote_entity_map
                                        .local_to_remote,
                            ) {
                                connection.messages_to_rebroadcast.push((
                                    reader.consume(),
                                    target,
                                    channel_kind,
                                ));
                            }
                        }
                        events.send(ServerReceiveMessage::new(message, *client_id));
                    }
                    Err(e) => {
                        error!(?e, "could not deserialize leafwing input message");
                    }
                }
            })
        }
    }
}

/// Read the InputState for the current tick from the buffer, and use them to update the ActionState
fn update_action_state<A: LeafwingUserAction>(
    tick_manager: Res<TickManager>,
    // global_input_buffer: Res<InputBuffer<A>>,
    // global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<(Entity, &mut ActionState<A>, &mut InputBuffer<A>)>,
) {
    let tick = tick_manager.tick();

    for (entity, mut action_state, mut input_buffer) in action_state_query.iter_mut() {
        // We only apply the ActionState from the buffer if we have one.
        // If we don't (because the input packet is late or lost), we won't do anything.
        // This is equivalent to considering that the player will keep playing the last action they played.
        if let Some(action) = input_buffer.get(tick) {
            *action_state = action.clone();
            trace!(?tick, ?entity, pressed = ?action_state.get_pressed(), "action state after update. Input Buffer: {}", input_buffer.as_ref());
            // remove all the previous values
            // we keep the current value in the InputBuffer so that if future messages are lost, we can still
            // fallback on the last known value
            input_buffer.pop(tick - 1);

            #[cfg(feature = "metrics")]
            {
                // The size of the buffer should always bet at least 1, and hopefully be a bit more than that
                // so that we can handle lost messages
                metrics::gauge!(format!(
                    "inputs::{}::{}::buffer_size",
                    std::any::type_name::<A>(),
                    entity
                ))
                .set(input_buffer.len() as f64);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inputs::leafwing::input_buffer::InputBuffer;
    use leafwing_input_manager::prelude::ActionState;

    use crate::prelude::client;
    use crate::prelude::server::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;

    #[test]
    fn test_leafwing_inputs() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let mut stepper = BevyStepper::default();

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

        // check that the server entity got a ActionDiffBuffer added to it
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .is_some());

        // check that the entity is replicated
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        // add an InputMap to the client entity, this should trigger the creation of an ActionState
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
        // check that the client entity got an InputBuffer added to it
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .is_some());

        // update the ActionState on the client by pressing on the button once
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        debug!("before press");
        stepper.frame_step();
        // client tick when we send the Jump action
        let client_tick = stepper.client_tick();
        // TODO: this test sometimes fails because the ActionState is not updated even after we call stepper.frame_step()
        //  i.e. action_state.get_pressed().is_empty()
        // we should have sent an InputMessage from client to server, and updated the input buffer on the server
        // for the client's tick
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick)
            .unwrap()
            .pressed(&LeafwingInput1::Jump));
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        // TODO: how come I need to frame_step() twice to see the release action?
        debug!("before release");
        stepper.frame_step();
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick + 1)
            .unwrap()
            .released(&LeafwingInput1::Jump));
    }
}
