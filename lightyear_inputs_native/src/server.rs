//! Handles client-generated inputs
use bevy::prelude::*;

use crate::client::config::ClientConfig;
use crate::connection::client::{ClientConnection, NetClient};
use crate::inputs::native::input_buffer::InputBuffer;
use crate::inputs::native::input_message::{InputMessage, InputTarget};
use crate::inputs::native::{ActionState, InputMarker};
use crate::prelude::{is_host_server, ChannelKind, ChannelRegistry, ClientConnectionManager, InputChannel, MessageRegistry, NetworkTarget, ServerReceiveMessage, ServerSendMessage, TickManager, UserAction};
use crate::server::connection::ConnectionManager;
use crate::server::input::InputSystemSet;
use crate::shared::input::InputConfig;
use tracing::{debug, trace};

pub struct InputPlugin<A> {
    /// If True, the server will rebroadcast a client's inputs to all other clients.
    ///
    /// It could be useful for a client to have access to other client's inputs to be able
    /// to predict their actions
    pub(crate) rebroadcast_inputs: bool,
    pub(crate) marker: core::marker::PhantomData<A>,
}

impl<A> Default for InputPlugin<A> {
    fn default() -> Self {
        Self {
            rebroadcast_inputs: false,
            marker: core::marker::PhantomData,
        }
    }
}

impl<A: UserAction> Plugin for InputPlugin<A> {
    fn build(&self, app: &mut App) {
        app.add_plugins(super::BaseInputPlugin::<ActionState<A>> {
            rebroadcast_inputs: self.rebroadcast_inputs,
            marker: core::marker::PhantomData,
        });
        // SYSTEMS
        // we don't need this for native inputs because InputBuffer is required by ActionState
        // app.add_observer(add_action_state_buffer::<A>);
        app.add_systems(
            PreUpdate,
            (receive_input_message::<A>,).in_set(InputSystemSet::ReceiveInputs),
        );

        // TODO: make this changeable dynamically by putting this in a resource?
        if self.rebroadcast_inputs {
            app.add_systems(
                PostUpdate,
                (
                    send_host_server_input_message::<A>.run_if(is_host_server),
                    rebroadcast_inputs::<A>,
                )
                    .chain()
                    .in_set(InputSystemSet::RebroadcastInputs),
            );
        }
    }
}

/// Read the input messages from the server events to update the InputBuffers
fn receive_input_message<A: UserAction>(
    message_registry: Res<MessageRegistry>,
    // we use an EventReader and not an event because the user might want to re-broadcast the inputs
    mut received_inputs: EventReader<ServerReceiveMessage<InputMessage<A>>>,
    connection_manager: Res<ConnectionManager>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    mut query: Query<Option<&mut InputBuffer<ActionState<A>>>>,
    mut commands: Commands,
) {
    received_inputs.read().for_each(|event| {
        let message = &event.message;
        let client_id = event.from;
        // ignore input messages from the local client (if running in host-server mode)
        if client_id.is_local() {
            return
        }
        trace!(?client_id, action = ?core::any::type_name::<A>(), ?message.end_tick, ?message.inputs, "received input message");

        // TODO: or should we try to store in a buffer the interpolation delay for the exact tick
        //  that the message was intended for?
        if let Some(interpolation_delay) = message.interpolation_delay {
            // update the interpolation delay estimate for the client
            if let Ok(client_entity) = connection_manager.client_entity(client_id) {
                commands.entity(client_entity).insert(interpolation_delay);
            }
        }

        for data in &message.inputs {
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
                            buffer.update_from_message(message.end_tick, &data.states);
                            trace!(
                                "Updated InputBuffer: {} using InputMessage: {:?}",
                                buffer.as_ref(),
                                message
                            );
                        } else {
                            trace!("Adding InputBuffer and ActionState which are missing on the entity");
                            let mut buffer = InputBuffer::<ActionState<A>>::default();
                            buffer.update_from_message(message.end_tick, &data.states);
                            commands.entity(entity).insert((
                                buffer,
                                ActionState::<A>::default(),
                            ));
                        }
                    } else {
                        debug!(?entity, ?data.states, end_tick = ?message.end_tick, "received input message for unrecognized entity");
                    }
                }
            }
        }
    });
}

/// In host-server mode, we usually don't need to send any input messages because any update
/// to the ActionState is immediately visible to the server.
/// However we might want other clients to see the inputs of the host client, in which case we will create
/// a InputMessage and send it to the server. The user can then have a `replicate_inputs` system that takes this
/// message and propagates it to other clients
fn send_host_server_input_message<A: UserAction>(
    connection: Res<ClientConnectionManager>,
    netclient: Res<ClientConnection>,
    mut events: ResMut<Events<ServerReceiveMessage<InputMessage<A>>>>,
    channel_registry: Res<ChannelRegistry>,
    config: Res<ClientConfig>,
    input_config: Res<InputConfig<A>>,
    tick_manager: Res<TickManager>,
    mut input_buffer_query: Query<(Entity, &mut InputBuffer<ActionState<A>>), With<InputMarker<A>>>,
) {
    // we send a message from the latest tick that we have available, which is the delayed tick
    let current_tick = tick_manager.tick();
    let input_delay_ticks = connection.input_delay_ticks() as i16;
    let tick = current_tick + input_delay_ticks;
    // TODO: the number of messages should be in SharedConfig
    trace!(tick = ?tick, "prepare_input_message");
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

    events.send(ServerReceiveMessage::new(message, netclient.id()));
}

pub(crate) fn rebroadcast_inputs<A: UserAction>(
    mut receive_inputs: ResMut<Events<ServerReceiveMessage<InputMessage<A>>>>,
    mut send_inputs: EventWriter<ServerSendMessage<InputMessage<A>>>,
) {
    // rebroadcast the input to other clients
    // we are calling drain() here so make sure that this system runs after the `ReceiveInputs` set,
    // so that the server had the time to process the inputs
    send_inputs.write_batch(receive_inputs.drain().map(|ev| {
        ServerSendMessage::new_with_target::<InputChannel>(
            ev.message,
            NetworkTarget::AllExceptSingle(ev.from),
        )
    }));
}
