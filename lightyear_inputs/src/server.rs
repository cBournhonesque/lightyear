//! Handle input messages received from the clients

use crate::input_buffer::InputBuffer;
use crate::input_message::{ActionStateSequence, InputMessage, InputTarget};
use crate::plugin::InputPlugin;
use crate::{HISTORY_DEPTH, InputChannel};
#[cfg(feature = "metrics")]
use alloc::format;
use bevy_app::{App, FixedPreUpdate, Plugin, PreUpdate};
use bevy_ecs::prelude::Has;
use bevy_ecs::{
    entity::{Entity, MapEntities},
    error::Result,
    query::With,
    resource::Resource,
    schedule::{IntoScheduleConfigs, SystemSet},
    system::{Commands, Query, Res, Single, StaticSystemParam},
};
use lightyear_connection::client::Connected;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::host::HostServer;
use lightyear_connection::prelude::NetworkTarget;
use lightyear_connection::server::Started;
use lightyear_core::id::RemoteId;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_link::prelude::{LinkOf, Server};
use lightyear_messages::plugin::MessageSet;
use lightyear_messages::prelude::MessageReceiver;
use lightyear_messages::server::ServerMultiMessageSender;
use tracing::{debug, error, trace};

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

#[derive(Resource)]
struct ServerInputConfig<S> {
    rebroadcast_inputs: bool,
    pub marker: core::marker::PhantomData<S>,
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSet {
    /// Receive the latest ActionDiffs from the client
    ReceiveInputs,
    /// Use the ActionDiff received from the client to update the `ActionState`
    UpdateActionState,
}

impl<S: ActionStateSequence + MapEntities> Plugin for ServerInputPlugin<S> {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<InputPlugin<S>>() {
            app.add_plugins(InputPlugin::<S>::default());
        }
        app.insert_resource::<ServerInputConfig<S>>(ServerInputConfig::<S> {
            // TODO: make this changeable dynamically by putting this in a resource?
            rebroadcast_inputs: self.rebroadcast_inputs,
            marker: core::marker::PhantomData,
        });

        // SETS
        // TODO:
        //  - could there be an issue because, client updates `state` and `fixed_update_state` and sends it to server
        //  - server only considers `state` since we receive messages in PreUpdate
        //  - but host-server broadcasting their inputs only updates `state`
        app.configure_sets(
            PreUpdate,
            (MessageSet::Receive, InputSet::ReceiveInputs).chain(),
        );
        app.configure_sets(FixedPreUpdate, InputSet::UpdateActionState);

        // for host server mode?
        #[cfg(feature = "client")]
        app.configure_sets(
            FixedPreUpdate,
            InputSet::UpdateActionState.after(crate::client::InputSet::BufferClientInputs),
        );

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            receive_input_message::<S>.in_set(InputSet::ReceiveInputs),
        );
        app.add_systems(
            FixedPreUpdate,
            update_action_state::<S>.in_set(InputSet::UpdateActionState),
        );
    }
}

// TODO: why do we need the Server? we could just run this on any receiver.
//  (apart from rebroadcast inputs)

/// Read the input messages from the server events to update the InputBuffers
fn receive_input_message<S: ActionStateSequence>(
    config: Res<ServerInputConfig<S>>,
    server: Query<&Server>,
    context: StaticSystemParam<S::Context>,
    mut sender: ServerMultiMessageSender,
    mut receivers: Query<
        (
            Entity,
            &LinkOf,
            &ClientOf,
            &mut MessageReceiver<InputMessage<S>>,
            &RemoteId,
        ),
        // We also receive inputs from the HostClient, in case we want the HostClient's inputs to be
        // rebroadcast to other clients (so that they can do prediction of the HostClient's entity)
        With<Connected>,
    >,
    mut query: Query<Option<&mut InputBuffer<S::Snapshot>>>,
    mut commands: Commands,
) -> Result {
    // TODO: use par_iter_mut
    receivers.iter_mut().try_for_each(|(client_entity, link_of, client_of, mut receiver, client_id)| {
        // TODO: this drains the messages... but the user might want to re-broadcast them?
        //  should we just read instead?
        let server_entity = link_of.server;
        receiver.receive().try_for_each(|message| {
            // ignore input messages from the local client (if running in host-server mode)
            // if we're not doing rebroadcasting
            if client_id.is_local() && !config.rebroadcast_inputs {
                error!("Received input message from HostClient for action {:?} even though rebroadcasting is disabled. Ignoring the message.", core::any::type_name::<S::Action>());
                return Ok(())
            }
            trace!(?client_id, action = ?core::any::type_name::<S::Action>(), ?message.end_tick, ?message.inputs, "received input message");

            // TODO: or should we try to store in a buffer the interpolation delay for the exact tick
            //  that the message was intended for?
            #[cfg(feature = "interpolation")]
            if let Some(interpolation_delay) = message.interpolation_delay {
                // update the interpolation delay estimate for the client
                commands.entity(client_entity).insert(interpolation_delay);
            }

            #[cfg(feature = "prediction")]
            if config.rebroadcast_inputs {
                trace!("Rebroadcast input message {message:?} from client {client_id:?} to other clients");
                if let Ok(server) = server.get(server_entity) {
                    sender.send::<_, InputChannel>(
                        &message,
                        server,
                        &NetworkTarget::AllExceptSingle(client_id.0)
                    )?;
                }
            }

            for data in message.inputs {
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
                                    "Updating InputBuffer: {} using: {:?}",
                                    buffer.as_ref(),
                                    data.states
                                );
                                data.states.update_buffer(&mut buffer, message.end_tick);
                            } else {
                                debug!("Adding InputBuffer and ActionState which are missing on the entity");
                                let mut buffer = InputBuffer::<S::Snapshot>::default();
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
            Ok(())
        })
    })
}

/// Read the InputState for the current tick from the buffer, and use them to update the ActionState
///
/// NOTE: this will also run on HostClients! This is why we disable `get_action_state` in the client
/// plugin for host-clients. This system also removes old inputs from the buffer, which is why we
/// can also skip `clear_buffers` on host-clients
fn update_action_state<S: ActionStateSequence>(
    context: StaticSystemParam<S::Context>,
    // TODO: what if there are multiple servers? maybe we can use Replicate to figure out which inputs should be replicating on which servers?
    //  and use the timeline from that connection? i.e. find from which entity we got the first InputMessage?
    //  presumably the entity is replicated to many clients, but only one client is controlling the entity?
    server: Single<(Entity, &LocalTimeline, Has<HostServer>), With<Started>>,
    mut action_state_query: Query<(Entity, &mut S::State, &mut InputBuffer<S::Snapshot>)>,
) {
    let (server, timeline, host_client) = server.into_inner();
    let tick = timeline.tick();
    for (entity, mut action_state, mut input_buffer) in action_state_query.iter_mut() {
        trace!(?tick, ?server, ?input_buffer, "input buffer on server");
        // We only apply the ActionState from the buffer if we have one.
        // If we don't (because the input packet is late or lost), we won't do anything.
        // This is equivalent to considering that the player will keep playing the last action they played.
        if let Some(snapshot) = input_buffer.get(tick) {
            S::from_snapshot(action_state.as_mut(), snapshot, &context);
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
                    core::any::type_name::<S::Action>(),
                    entity
                ))
                .set(input_buffer.len() as f64);
            }
        }

        // NOTE: if we are the host-client, it is important to keep some history in the inputs
        // The reason is that we are sending our inputs to other clients, which might cause rollbacks.
        // For example there are new inputs starting from tick 7: L, L, L
        // But the other clients might receive the message from tick 9 first (because of reordering), in which case it
        // is important that they know that the action L was first pressed at tick 7! If the history is cut too short,
        // then that information is not included in the message
        // Basically, in host-client we are producer of inputs, so we need to include some redundancy. (like when
        // normal clients send inputs)
        let history_depth = if host_client {
            HISTORY_DEPTH
        } else {
            // if we are a server and not a host-client, there is no need to keep history
            1
        };
        // TODO: + we also want to keep enough inputs on the client to be able to do prediction effectively!
        // remove all the previous values
        // we keep the current value in the InputBuffer so that if future messages are lost, we can still
        // fallback on the last known value
        input_buffer.pop(tick - history_depth);
    }
}
