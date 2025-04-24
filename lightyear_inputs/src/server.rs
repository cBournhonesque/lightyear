//! Handle input messages received from the clients
use crate::input_buffer::InputBuffer;
use crate::input_message::{ActionStateSequence, InputMessage, InputTarget};
use bevy::prelude::*;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::server::Started;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_messages::plugin::MessageSet;
use lightyear_messages::prelude::MessageReceiver;
use tracing::trace;

pub struct ServerInputPlugin<S> {
    pub rebroadcast_inputs: bool,
    pub marker: core::marker::PhantomData<S>,
}

impl<S> Default for ServerInputPlugin<S> {
    fn default() -> Self {
        Self {
            rebroadcast_inputs: false,
            marker: core::marker::PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSet {
    /// Receive the latest ActionDiffs from the client
    ReceiveInputs,
    /// Use the ActionDiff received from the client to update the [`ActionState`]
    UpdateActionState,
    /// Rebroadcast inputs to other clients
    RebroadcastInputs,
}

impl<S: ActionStateSequence> Plugin for ServerInputPlugin<S> {
    fn build(&self, app: &mut App) {
        // SETS
        // TODO:
        //  - could there be an issue because, client updates `state` and `fixed_update_state` and sends it to server
        //  - server only considers `state`
        //  - but host-server broadcasting their inputs only updates `state`
        app.configure_sets(
            PreUpdate, (MessageSet::Receive, InputSet::ReceiveInputs).chain()
        );
        app.configure_sets(FixedPreUpdate, InputSet::UpdateActionState);

        // for host server mode?
        #[cfg(feature = "client")]
        app.configure_sets(FixedPreUpdate, InputSet::UpdateActionState.after(
            crate::client::InputSet::BufferClientInputs
        ));

        // TODO: maybe put this in a Fixed schedule to avoid sending multiple host-server identical
        //  messages per frame if we didn't run FixedUpdate at all?
        app.configure_sets(PostUpdate, InputSet::RebroadcastInputs.before(MessageSet::Send));

        // SYSTEMS
        app.add_systems(
            PreUpdate, receive_input_message::<S>.in_set(InputSet::ReceiveInputs),
        );
        app.add_systems(
            FixedPreUpdate,
            update_action_state::<S>.in_set(InputSet::UpdateActionState),
        );

        // // TODO: make this changeable dynamically by putting this in a resource?
        // if self.rebroadcast_inputs {
        //     app.add_systems(
        //         PostUpdate,
        //         (
        //             send_host_server_input_message::<A>.run_if(is_host_server),
        //             rebroadcast_inputs::<A>,
        //         )
        //             .chain()
        //             .in_set(InputSet::RebroadcastInputs),
        //     );
        // }

    }
}

/// Read the input messages from the server events to update the InputBuffers
fn receive_input_message<S: ActionStateSequence>(
    mut receivers: Query<(&ClientOf, &mut MessageReceiver<InputMessage<S>>)>,
    mut query: Query<Option<&mut InputBuffer<S::State>>>,
    mut commands: Commands,
) {
    // TODO: use par_iter_mut
    receivers.iter_mut().for_each(|(client_of, mut receiver)| {
        // TODO: this drains the messages... but the user might want to re-broadcast them?
        //  should we just read insteaD?
        let client_id = client_of.id;
        receiver.receive().for_each(|message| {
            // ignore input messages from the local client (if running in host-server mode)
            if client_id.is_local() {
                return
            }
            trace!(?client_id, action = ?core::any::type_name::<S::Action>(), ?message.end_tick, ?message.inputs, "received input message");

            // // TODO: or should we try to store in a buffer the interpolation delay for the exact tick
            // //  that the message was intended for?
            // if let Some(interpolation_delay) = message.interpolation_delay {
            //     // update the interpolation delay estimate for the client
            //     if let Ok(client_entity) = connection_manager.client_entity(client_id) {
            //         commands.entity(client_entity).insert(interpolation_delay);
            //     }
            // }

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
                                data.states.update_buffer(&mut buffer, message.end_tick);
                                trace!(
                                    "Updated InputBuffer: {} using InputMessage: {:?}",
                                    buffer.as_ref(),
                                    message
                                );
                            } else {
                                trace!("Adding InputBuffer and ActionState which are missing on the entity");
                                let mut buffer = InputBuffer::<S::State>::default();
                                data.states.update_buffer(&mut buffer, message.end_tick);
                                commands.entity(entity).insert((
                                    buffer,
                                    S::State::default()
                                ));
                                // commands.command_scope(|mut commands| {
                                //     commands.entity(entity).insert((
                                //         buffer,
                                //         ActionState::<A>::default(),
                                //     ));
                                // });
                            }
                        } else {
                            debug!(?entity, ?data.states, end_tick = ?message.end_tick, "received input message for unrecognized entity");
                        }
                    }
                }
            }
        })
    });
}

/// Read the InputState for the current tick from the buffer, and use them to update the ActionState
fn update_action_state<S: ActionStateSequence>(
    // TODO: what if there are multiple servers? maybe we can use Replicate to figure out which inputs should be replicating on which servers?
    //  and use the timeline from that connection? i.e. find from which entity we got the first InputMessage?
    //  presumably the entity is replicated to many clients, but only one client is controlling the entity?
    server: Query<(Entity, &LocalTimeline), With<Started>>,
    mut action_state_query: Query<(Entity, &mut S::State, &mut InputBuffer<S::State>)>,
) {
    let Ok((server, timeline)) = server.single() else {
        // We don't have a server timeline, so we can't update the action state
        return;
    };

    let tick = timeline.tick();
    for (entity, mut action_state, mut input_buffer) in action_state_query.iter_mut() {
        trace!(?tick, ?server, ?input_buffer, "input buffer on server");
        // We only apply the ActionState from the buffer if we have one.
        // If we don't (because the input packet is late or lost), we won't do anything.
        // This is equivalent to considering that the player will keep playing the last action they played.
        if let Some(action) = input_buffer.get(tick) {
            *action_state = action.clone();
            trace!(
                ?tick,
                ?entity,
                "action state after update. Input Buffer: {}",
                input_buffer.as_ref()
            );

            #[cfg(feature = "metrics")]
            {
                // The size of the buffer should always bet at least 1, and hopefully be a bit more than that
                // so that we can handle lost messages
                metrics::gauge!(format!(
                    "inputs::{}::{}::buffer_size",
                    core::any::type_name::<A>(),
                    entity
                ))
                .set(input_buffer.len() as f64);
            }
        }
        // TODO: in host-server mode, if we rebroadcast inputs, we might want to keep a bit of a history
        //  in the buffer so that we have redundancy when we broadcast to other clients
        // remove all the previous values
        // we keep the current value in the InputBuffer so that if future messages are lost, we can still
        // fallback on the last known value
        input_buffer.pop(tick - 1);
    }
}

// /// In host-server mode, we usually don't need to send any input messages because any update
// /// to the ActionState is immediately visible to the server.
// /// However we might want other clients to see the inputs of the host client, in which case we will create
// /// a InputMessage and send it to the server. The user can then have a `replicate_inputs` system that takes this
// /// message and propagates it to other clients
// fn send_host_server_input_message<A: UserAction>(
//     connection: Res<ClientConnectionManager>,
//     netclient: Res<ClientConnection>,
//     mut events: ResMut<Events<ServerReceiveMessage<InputMessage<A>>>>,
//     channel_registry: Res<ChannelRegistry>,
//     config: Res<ClientConfig>,
//     input_config: Res<InputConfig<A>>,
//     tick_manager: Res<TickManager>,
//     mut input_buffer_query: Query<(Entity, &mut InputBuffer<ActionState<A>>), With<InputMarker<A>>>,
// ) {
//     // we send a message from the latest tick that we have available, which is the delayed tick
//     let current_tick = tick_manager.tick();
//     let input_delay_ticks = connection.input_delay_ticks() as i16;
//     let tick = current_tick + input_delay_ticks;
//     // TODO: the number of messages should be in SharedConfig
//     trace!(tick = ?tick, "prepare_input_message");
//     // TODO: instead of redundancy, send ticks up to the latest yet ACK-ed input tick
//     //  this means we would also want to track packet->message acks for unreliable channels as well, so we can notify
//     //  this system what the latest acked input tick is?
//     let input_send_interval = channel_registry
//         .get_builder_from_kind(&ChannelKind::of::<InputChannel>())
//         .unwrap()
//         .settings
//         .send_frequency;
//     // we send redundant inputs, so that if a packet is lost, we can still recover
//     // A redundancy of 2 means that we can recover from 1 lost packet
//     let mut num_tick: u16 =
//         ((input_send_interval.as_nanos() / config.shared.tick.tick_duration.as_nanos()) + 1)
//             .try_into()
//             .unwrap();
//     num_tick *= input_config.packet_redundancy;
//     let mut message = InputMessage::<A>::new(tick);
//     for (entity, input_buffer) in input_buffer_query.iter_mut() {
//         trace!(
//             ?tick,
//             ?current_tick,
//             ?entity,
//             "Preparing host-server input message with buffer: {:?}",
//             input_buffer
//         );
//         // we are using PrePredictedEntity to make sure that MapEntities will be used on the client receiving side
//         message.add_inputs(
//             num_tick,
//             InputTarget::PrePredictedEntity(entity),
//             input_buffer.as_ref(),
//         );
//     }
//
//     events.send(ServerReceiveMessage::new(message, netclient.id()));
// }

// pub(crate) fn rebroadcast_inputs<A: UserAction>(
//     mut receive_inputs: ResMut<Events<ServerReceiveMessage<InputMessage<A>>>>,
//     mut send_inputs: EventWriter<ServerSendMessage<InputMessage<A>>>,
// ) {
//     // rebroadcast the input to other clients
//     // we are calling drain() here so make sure that this system runs after the `ReceiveInputs` set,
//     // so that the server had the time to process the inputs
//     send_inputs.write_batch(receive_inputs.drain().map(|ev| {
//         ServerSendMessage::new_with_target::<InputChannel>(
//             ev.message,
//             NetworkTarget::AllExceptSingle(ev.from),
//         )
//     }));
// }