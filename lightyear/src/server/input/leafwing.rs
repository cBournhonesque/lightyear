//! Handles client-generated inputs

use crate::client::config::ClientConfig;
use crate::connection::client::ClientConnection;
use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::inputs::leafwing::input_message::InputTarget;
use crate::inputs::leafwing::LeafwingUserAction;
use crate::prelude::client::NetClient;
use crate::prelude::{
    is_host_server, ChannelKind, ChannelRegistry, ClientConnectionManager, InputChannel,
    InputConfig, InputMessage, MessageRegistry, NetworkTarget, ServerReceiveMessage,
    ServerSendMessage, TickManager,
};
use crate::server::connection::ConnectionManager;
pub(crate) use crate::server::input::InputSystemSet;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

pub struct LeafwingInputPlugin<A> {
    pub(crate) rebroadcast_inputs: bool,
    pub(crate) marker: core::marker::PhantomData<A>,
}

impl<A> Default for LeafwingInputPlugin<A> {
    fn default() -> Self {
        Self {
            rebroadcast_inputs: false,
            marker: core::marker::PhantomData,
        }
    }
}

impl<A: LeafwingUserAction> Plugin for LeafwingInputPlugin<A> {
    fn build(&self, app: &mut App) {
        app.add_plugins(super::BaseInputPlugin::<ActionState<A>> {
            rebroadcast_inputs: self.rebroadcast_inputs,
            marker: core::marker::PhantomData,
        });

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            receive_input_message::<A>.in_set(InputSystemSet::ReceiveInputs),
        );

        // TODO: make this changeable dynamically by putting this in a resource?
        if self.rebroadcast_inputs {
            app.add_systems(
                PostUpdate,
                (
                    // TODO: is this necessary? why don't we just use the client's SendInputMessage?
                    //  now messages work seamlessly in host-server mode, so it should work!
                    send_host_server_input_message::<A>.run_if(is_host_server),
                    rebroadcast_inputs::<A>,
                )
                    .chain()
                    .in_set(InputSystemSet::RebroadcastInputs),
            );
        }
    }

    // TODO: this doesn't work! figure out how to make sure that InputManagerPlugin is called
    fn finish(&self, app: &mut App) {
        if !app.is_plugin_added::<InputManagerPlugin<A>>() {
            app.add_plugins(InputManagerPlugin::<A>::server());
        }
    }
}


// TODO? is this correct? maybe she would update the FixedUpdate state! not the Update state?
/// Read the input messages from the server events to update the InputBuffers
fn receive_input_message<A: LeafwingUserAction>(
    message_registry: Res<MessageRegistry>,
    // we use an EventReader and not an event because the user might want to re-broadcast the inputs
    mut received_inputs: EventReader<ServerReceiveMessage<InputMessage<A>>>,
    connection_manager: Res<ConnectionManager>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    mut query: Query<Option<&mut InputBuffer<A>>>,
    mut commands: Commands,
    tick_manager: Res<TickManager>,
) {
    let tick = tick_manager.tick();
    received_inputs.read().for_each(|event| {
        let message = &event.message;
        let client_id = event.from;
        trace!(?client_id, action = ?A::short_type_path(), ?message.end_tick, ?message.diffs, "received input message");

        // TODO: or should we try to store in a buffer the interpolation delay for the exact tick
        //  that the message was intended for?
        if let Some(interpolation_delay) = message.interpolation_delay {
            // update the interpolation delay estimate for the client
            if let Ok(client_entity) = connection_manager.client_entity(client_id) {
                commands.entity(client_entity).insert(interpolation_delay);
            }
        }

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
                                "Update InputBuffer: {} using InputMessage: {}",
                                buffer.as_ref(),
                                message
                            );
                            buffer.update_from_diffs(
                                message.end_tick,
                                &data.start_state,
                                &data.diffs,
                            );
                        } else {
                            debug!("Adding InputBuffer and ActionState which are missing on the entity");
                            let mut buffer = InputBuffer::<A>::default();
                            buffer.update_from_diffs(
                                message.end_tick,
                                &data.start_state,
                                &data.diffs,
                            );
                            commands.entity(entity).insert((
                                buffer,
                                ActionState::<A>::default(),
                            ));
                        }
                    } else {
                        debug!(?entity, ?data.diffs, end_tick = ?message.end_tick, "received input message for unrecognized entity");
                    }
                }
            }
        }
    });
}

/// In host-server mode, we usually don't need to send any input messages because any update
/// to the ActionState is immediately visible to the server.
/// However we might want other clients to see the inputs of the host client, in which case we will create
/// a InputMessage and send it to the server.
/// Then the 'rebroadcast_inputs' system will be able to rebroadcast the host-server's inputs to other clients.
fn send_host_server_input_message<A: LeafwingUserAction>(
    connection: Res<ClientConnectionManager>,
    netclient: Res<ClientConnection>,
    mut events: ResMut<Events<ServerReceiveMessage<InputMessage<A>>>>,
    channel_registry: Res<ChannelRegistry>,
    config: Res<ClientConfig>,
    input_config: Res<InputConfig<A>>,
    tick_manager: Res<TickManager>,
    mut input_buffer_query: Query<(Entity, &mut InputBuffer<A>), With<InputMap<A>>>,
) {
    // we send a message from the latest tick that we have available, which is the delayed tick
    let current_tick = tick_manager.tick();
    let input_delay_ticks = connection.input_delay_ticks() as i16;
    let tick = current_tick + input_delay_ticks;
    // TODO: the number of messages should be in SharedConfig
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
            .try_into()
            .unwrap();
    num_tick *= input_config.packet_redundancy;
    let mut message = InputMessage::<A>::new(tick);
    for (entity, input_buffer) in input_buffer_query.iter_mut() {
        trace!(
            ?tick,
            ?current_tick,
            ?entity,
            "Preparing host-server input message with buffer: {:?}",
            input_buffer
        );
        // we are using PrePredictedEntity to make sure that MapEntities will be used on the client receiving side
        message.add_inputs(
            num_tick,
            InputTarget::PrePredictedEntity(entity),
            input_buffer.as_ref(),
        );
    }
    trace!(
        ?tick,
        ?current_tick,
        %message,
        "Sending host-server input message"
    );

    events.send(ServerReceiveMessage::new(message, netclient.id()));
}

pub(crate) fn rebroadcast_inputs<A: LeafwingUserAction>(
    mut receive_inputs: ResMut<Events<ServerReceiveMessage<InputMessage<A>>>>,
    mut send_inputs: EventWriter<ServerSendMessage<InputMessage<A>>>,
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
