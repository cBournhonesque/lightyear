//! Handles client-generated inputs
use crate::client::prediction::{Rollback, RollbackState};
use crate::connection::events::IterInputMessageEvent;
use crate::inputs::leafwing::input_buffer::{ActionDiffBuffer, InputBuffer};
use crate::inputs::leafwing::{InputMessage, UserAction};
use bevy::prelude::*;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use std::ops::DerefMut;

use crate::netcode::ClientId;
use crate::prelude::MainSet;
use crate::protocol::Protocol;
use crate::server::events::InputMessageEvent;
use crate::server::resource::Server;
use crate::server::systems::receive;
use crate::shared::events::InputEvent;
use crate::shared::sets::FixedUpdateSet;

pub struct LeafwingInputPlugin<P: Protocol, A: UserAction> {
    protocol_marker: std::marker::PhantomData<P>,
    input_marker: std::marker::PhantomData<A>,
}

impl<P: Protocol, A: UserAction> Default for LeafwingInputPlugin<P, A> {
    fn default() -> Self {
        Self {
            protocol_marker: std::marker::PhantomData,
            input_marker: std::marker::PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    /// Use the ActionDiff received from the client to update the ActionState
    Update,
}

impl<P: Protocol, A: UserAction> Plugin for LeafwingInputPlugin<P, A>
where
    P::Message: TryInto<InputMessage<A>, Error = ()>,
{
    fn build(&self, app: &mut App) {
        // EVENTS
        app.add_event::<InputMessageEvent<A>>();
        // RESOURCES
        // TODO: add a resource tracking the action-state of all clients
        // PLUGINS
        // NOTE: we do note add the leafwing server plugin because it just ticks Action-States
        // SETS
        app.configure_sets(
            FixedUpdate,
            (
                FixedUpdateSet::TickUpdate,
                InputSystemSet::Update,
                FixedUpdateSet::Main,
            )
                .chain(),
        );
        // SYSTEMS
        app.add_systems(
            PreUpdate,
            // TODO: ideally we have a Flush between add_action_diff_buffer and Tick?
            // TODO: we could get: ActionState from client, so we need to handle inputs after ReceiveFlush, but before
            (
                add_action_diff_buffer::<A>,
                add_input_message_event::<P, A>,
                update_action_diff_buffers::<P, A>,
            )
                .chain()
                .after(MainSet::ReceiveFlush),
        );
        app.add_systems(
            FixedUpdate,
            update_action_state::<P, A>.in_set(InputSystemSet::Update),
        );
    }
}

/// For each entity that has an action-state, insert an action-state-buffer
/// that will store the value of the action-state for the last few ticks
fn add_action_diff_buffer<A: UserAction>(
    mut commands: Commands,
    action_state: Query<Entity, Added<ActionState<A>>>,
) {
    for entity in action_state.iter() {
        commands
            .entity(entity)
            .insert(ActionDiffBuffer::<A>::default());
    }
}

// Write the input messages from the server events to the Events
fn add_input_message_event<P: Protocol, A: UserAction>(
    mut server: ResMut<Server<P>>,
    mut input_message_events: ResMut<Events<InputMessageEvent<A>>>,
) where
    P::Message: TryInto<InputMessage<A>, Error = ()>,
{
    if server.events().has_input_messages::<A>() {
        for (message, client_id) in server.events().into_iter_input_messages::<A>() {
            input_message_events.send(InputMessageEvent::new(message, client_id));
        }
    }
}

fn update_action_diff_buffers<P: Protocol, A: UserAction>(
    // mut global: Option<ResMut<ActionDiffBuffer<A>>>,
    mut input_message: ResMut<Events<InputMessageEvent<A>>>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    mut query: Query<&mut ActionDiffBuffer<A>>,
) {
    for event in input_message.update_drain() {
        let mut message = event.message;
        // TODO: handle global diffs for each client! How? create one entity per client?
        //  or have a resource containing the global ActionState for each client?
        // if let Some(ref mut buffer) = global {
        //     buffer.update_from_message(message.end_tick, std::mem::take(&mut message.global_diffs))
        // }
        for (entity, diffs) in std::mem::take(&mut message.per_entity_diffs) {
            if let Ok(mut buffer) = query.get_mut(entity) {
                buffer.update_from_message(message.end_tick, diffs);
            }
        }
    }
}

// Read the ActionDiff for the current tick from the buffer, and use them to update the ActionState
fn update_action_state<P: Protocol, A: UserAction>(
    server: Res<Server<P>>,
    // global_input_buffer: Res<InputBuffer<A>>,
    // global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<(&mut ActionState<A>, &mut ActionDiffBuffer<A>)>,
) {
    let tick = server.tick();

    for (mut action_state, mut action_diff_buffer) in action_state_query.iter_mut() {
        action_diff_buffer
            .pop(tick)
            .into_iter()
            .for_each(|action| action.apply(action_state.deref_mut()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::client::{InterpolationConfig, PredictionConfig, SyncConfig};
    use crate::prelude::server::*;
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};
    use std::time::Duration;

    use leafwing_input_manager::prelude::ActionState;

    #[test]
    fn test_leafwing_inputs() {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            enable_replication: true,
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(0),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default().disable(false);
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = BevyStepper::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            link_conditioner,
            frame_duration,
        );
        stepper
            .client_app
            .add_plugins(crate::client::input_leafwing::LeafwingInputPlugin::<
                MyProtocol,
                LeafwingInput1,
            >::default());
        stepper
            .server_app
            .add_plugins(LeafwingInputPlugin::<MyProtocol, LeafwingInput1>::default());
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

        // check that the entity is replicated, including the ActionState component
        let client_entity = *stepper
            .client()
            .connection()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<ActionState<LeafwingInput1>>()
                .unwrap(),
            &ActionState::<LeafwingInput1>::default()
        );
        // check that the client entity got an InputBuffer added to it
        assert!(stepper
            .server_app
            .world
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .is_some());

        // update the ActionState on the client
        stepper
            .client_app
            .world
            .entity_mut(client_entity)
            .get_mut::<ActionState<LeafwingInput1>>()
            .unwrap()
            .press(LeafwingInput1::Jump);
        stepper.frame_step();
    }
}
