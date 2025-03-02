//! Handles client-generated inputs
use crate::inputs::native::input_buffer::InputBuffer;
use crate::inputs::native::input_message::{InputMessage, InputTarget};
use crate::inputs::native::ActionState;
use crate::prelude::{is_host_server, MessageRegistry, ServerReceiveMessage, UserAction};
use crate::server::connection::ConnectionManager;
use crate::server::input::InputSystemSet;
use bevy::prelude::*;

pub struct InputPlugin<A> {
    marker: std::marker::PhantomData<A>,
}

impl<A> Default for InputPlugin<A> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}


impl<A: UserAction> Plugin for InputPlugin<A> {
    fn build(&self, app: &mut App) {
        app.add_plugins(super::BaseInputPlugin::<ActionState<A>>::default());
        // SYSTEMS
        app.add_systems(
            PreUpdate,
                receive_input_message::<A>.in_set(InputSystemSet::ReceiveInputs)
        );
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
        error!(?client_id, action = ?std::any::type_name::<A>(), ?message.end_tick, ?message.inputs, "received input message");

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
                    error!("received input for entity: {:?}", entity);

                    if let Ok(buffer) = query.get_mut(entity) {
                        if let Some(mut buffer) = buffer {
                            error!(
                                "Update InputBuffer: {} using InputMessage: {:?}",
                                buffer.as_ref(),
                                message
                            );
                            buffer.update_from_message(message.end_tick, &data.states);
                        } else {
                            error!("Adding InputBuffer and ActionState which are missing on the entity");
                            commands.entity(entity).insert((
                                InputBuffer::<ActionState<A>>::default(),
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