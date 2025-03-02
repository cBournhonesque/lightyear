//! Handles client-generated inputs
use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::inputs::leafwing::input_message::InputTarget;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

use crate::inputs::leafwing::LeafwingUserAction;
use crate::inputs::native::UserActionState;
use crate::prelude::{is_host_server, InputMessage, MessageRegistry, ServerReceiveMessage};
use crate::server::connection::ConnectionManager;
use crate::server::input::InputSystemSet;

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


impl<A: LeafwingUserAction> Plugin for LeafwingInputPlugin<A> {
    fn build(&self, app: &mut App) {
        app.add_plugins(super::BaseInputPlugin::<ActionState<A>>::default());


        // SYSTEMS
        // TODO: this runs twice in host-server mode. How to avoid this?
        app.add_observer(add_action_state_buffer::<A>);
        app.add_systems(
            PreUpdate,
                receive_input_message::<A>.in_set(InputSystemSet::ReceiveInputs)
        );
    }

    // TODO: this doesn't work! figure out how to make sure that InputManagerPlugin is called
    fn finish(&self, app: &mut App) {
        if !app.is_plugin_added::<InputManagerPlugin<A>>() {
            app.add_plugins(InputManagerPlugin::<A>::server());
        }
    }
}

/// For each entity that has the Action component, insert an input buffer.
fn add_action_state_buffer<A: LeafwingUserAction>(
    trigger: Trigger<OnAdd, ActionState<A>>,
    mut commands: Commands,
    query: Query<(), Without<InputBuffer<A>>>,
) {
    if let Ok(()) = query.get(trigger.entity()) {
        commands.entity(trigger.entity()).insert((
            InputBuffer::<A>::default(),
        ));
    }
}

/// Read the input messages from the server events to update the InputBuffers
fn receive_input_message<A: LeafwingUserAction>(
    message_registry: Res<MessageRegistry>,
    // we use an EventReader and not an event because the user might want to re-broadcast the inputs
    mut received_inputs: EventReader<ServerReceiveMessage<InputMessage<A>>>,
    connection_manager: Res<ConnectionManager>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    mut query: Query<Option<&mut InputBuffer<A>>>,
    mut commands: Commands,
) {
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
                            commands.entity(entity).insert((
                                InputBuffer::<A>::default(),
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
