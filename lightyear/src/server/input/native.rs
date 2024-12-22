//! Handles client-generated inputs
use bevy::prelude::*;
use bevy::utils::HashMap;

use crate::inputs::native::input_buffer::InputBuffer;
use crate::inputs::native::InputMessage;
use crate::prelude::server::DisconnectEvent;
use crate::prelude::{server::is_started, ClientId, MessageRegistry, TickManager, UserAction};
use crate::protocol::message::MessageKind;
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::server::events::InputEvent;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{InternalMainSet, ServerMarker};

pub struct InputPlugin<A: UserAction> {
    _marker: std::marker::PhantomData<A>,
}

#[derive(Resource, Debug)]
pub struct InputBuffers<A> {
    /// The first element stores the last input we have received from the client.
    /// In case we are missing the client input for a tick, we will fallback to using this.
    buffers: HashMap<ClientId, (Option<A>, InputBuffer<A>)>,
}

impl<A> Default for InputBuffers<A> {
    fn default() -> Self {
        Self {
            buffers: HashMap::default(),
        }
    }
}

impl<A: UserAction> Default for InputPlugin<A> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    /// PreUpdate system where we receive and deserialize the InputMessage
    ReceiveInputMessage,
    /// FixedUpdate system to get any inputs from the client. This should be run before the game/physics logic
    WriteInputEvents,
    /// System Set to clear the input events (otherwise bevy clears events every frame, not every tick)
    ClearInputEvents,
}

impl<A: UserAction> Plugin for InputPlugin<A> {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.init_resource::<InputBuffers<A>>();
        // EVENTS
        app.add_event::<InputEvent<A>>();
        // SETS
        app.configure_sets(
            PreUpdate,
            InputSystemSet::ReceiveInputMessage
                .in_set(InternalMainSet::<ServerMarker>::EmitEvents)
                .run_if(is_started),
        );
        app.configure_sets(
            FixedPreUpdate,
            InputSystemSet::WriteInputEvents.run_if(is_started),
        );
        app.configure_sets(
            FixedPostUpdate,
            InputSystemSet::ClearInputEvents.run_if(is_started),
        );

        app.add_systems(
            PreUpdate,
            receive_input_message::<A>.in_set(InputSystemSet::ReceiveInputMessage),
        );
        app.add_systems(
            FixedPreUpdate,
            write_input_event::<A>.in_set(InputSystemSet::WriteInputEvents),
        );
        app.add_systems(
            FixedPostUpdate,
            clear_input_events::<A>.in_set(InputSystemSet::ClearInputEvents),
        );
        app.add_observer(handle_client_disconnect::<A>);
    }
}

/// Remove the client if the client disconnects
fn handle_client_disconnect<A: UserAction>(
    trigger: Trigger<DisconnectEvent>,
    mut input_buffers: ResMut<InputBuffers<A>>,
) {
    input_buffers.buffers.remove(&trigger.event().client_id);
}

/// Read the message received from the client and emit the MessageEvent event
fn receive_input_message<A: UserAction>(
    message_registry: Res<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
    mut input_buffers: ResMut<InputBuffers<A>>,
) {
    let kind = MessageKind::of::<InputMessage<A>>();
    let Some(net) = message_registry.kind_map.net_id(&kind).copied() else {
        error!(
            "Could not find the network id for the message kind: {:?}",
            kind
        );
        return;
    };
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        if let Some(message_list) = connection.received_input_messages.remove(&net) {
            for (message_bytes, target, channel_kind) in message_list {
                let mut reader = Reader::from(message_bytes);
                match message_registry.deserialize::<InputMessage<A>>(
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok(message) => {
                        debug!("Received input message: {:?}", message);
                        input_buffers
                            .buffers
                            .entry(*client_id)
                            .or_default()
                            .1
                            .update_from_message(message);
                        if target != NetworkTarget::None {
                            // NOTE: we can re-send the same bytes directly because InputMessage does not include any Entity references
                            connection.messages_to_rebroadcast.push((
                                // TODO: avoid clone, or use bytes?
                                reader.consume(),
                                target,
                                channel_kind,
                            ));
                        }
                    }
                    Err(e) => {
                        error!("Error deserializing input message: {:?}", e);
                    }
                }
            }
        }
    }
}

// Create a system that reads from the input buffer and returns the inputs of all clients for the current tick.
// The only tricky part is that events are cleared every frame, but we want to clear every tick instead
// Do it in this system because we want an input for every tick
fn write_input_event<A: UserAction>(
    tick_manager: Res<TickManager>,
    mut input_buffers: ResMut<InputBuffers<A>>,
    mut input_events: EventWriter<InputEvent<A>>,
) {
    let tick = tick_manager.tick();
    input_buffers
        .buffers
        .iter_mut()
        .for_each(move |(client_id, (last_input, input_buffer))| {
            debug!(?input_buffer, ?tick, ?client_id, "input buffer for client");
            let received_input = input_buffer.pop(tick);
            let fallback = received_input.is_none();

            // NOTE: if there is no input for this tick, we should use the last input that we have
            //  as a best-effort fallback.
            let input = match received_input {
                None => last_input.clone(),
                Some(i) => {
                    *last_input = Some(i.clone());
                    Some(i)
                }
            };
            if fallback {
                // TODO: do not log this while clients are syncing..
                debug!(
                ?client_id,
                ?tick,
                fallback_input = ?&input,
                "Missed client input!"
                )
            }
            // TODO: We should also let the user know that it needs to send inputs a bit earlier so that
            //  we have more of a buffer. Send a SyncMessage to tell the user to speed up?
            //  See Overwatch GDC video
            input_events.send(InputEvent::new(input, *client_id));
        });
}

/// System that clears the input events.
/// It is necessary because events are cleared every frame, but we want to clear every tick instead
fn clear_input_events<A: UserAction>(mut input_events: EventReader<InputEvent<A>>) {
    input_events.clear();
}
