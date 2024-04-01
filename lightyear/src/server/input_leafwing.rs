//! Handles client-generated inputs
use std::ops::DerefMut;

use crate::_reexport::ServerMarker;
use crate::client::components::Confirmed;
use crate::client::config::ClientConfig;
use crate::client::prediction::Predicted;
use crate::connection::client::NetClient;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

use crate::inputs::leafwing::input_buffer::{
    ActionDiffBuffer, ActionDiffEvent, InputBuffer, InputTarget,
};
use crate::inputs::leafwing::{InputMessage, LeafwingUserAction};
use crate::prelude::client::is_in_rollback;
use crate::prelude::{client, Mode, SharedConfig, TickManager};
use crate::protocol::Protocol;
use crate::server::config::ServerConfig;
use crate::server::connection::ConnectionManager;
use crate::server::events::InputMessageEvent;
use crate::shared::events::connection::IterInputMessageEvent;
use crate::shared::replication::components::PrePredicted;
use crate::shared::sets::InternalMainSet;

pub struct LeafwingInputPlugin<P, A> {
    marker: std::marker::PhantomData<(P, A)>,
}

// // TODO: also create events on top of this?
// /// Keeps tracks of the global ActionState<A> of every client on the given tick
// #[derive(Resource, Debug, Clone)]
// pub struct GlobalActions<A: LeafwingUserAction> {
//     inputs: HashMap<ClientId, ActionState<A>>,
// }

// impl<A: LeafwingUserAction> Default for GlobalActions<A> {
//     fn default() -> Self {
//         Self {
//             inputs: HashMap::default(),
//         }
//     }
// }

impl<P, A> Default for LeafwingInputPlugin<P, A> {
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

impl<P: Protocol, A: LeafwingUserAction> Plugin for LeafwingInputPlugin<P, A>
where
    P::Message: TryInto<InputMessage<A>, Error = ()>,
{
    fn build(&self, app: &mut App) {
        // EVENTS
        app.add_event::<InputMessageEvent<A>>();
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
                .chain(),
        );
        app.configure_sets(FixedPreUpdate, InputSystemSet::Update);
        // SYSTEMS
        app.add_systems(
            PreUpdate,
            (
                // TODO: ideally we have a Flush between add_action_diff_buffer and Tick?
                add_action_diff_buffer::<A>.in_set(InputSystemSet::AddBuffers),
                receive_input_message::<P, A>.in_set(InputSystemSet::ReceiveInputs),
            ),
        );
        app.add_systems(
            FixedPreUpdate,
            update_action_state::<A>.in_set(InputSystemSet::Update),
        );

        if app.world.resource::<ServerConfig>().shared.mode == Mode::HostServer {
            // in unified mode, we receive the action diffs directly from the ActionDiffEvent events
            app.configure_sets(
                FixedPreUpdate,
                InputSystemSet::ReceiveInputs
                    // be careful to read the ActionDiffEvents before they are consumed in the client's
                    // `write_action_diffs` system
                    .before(client::InputSystemSet::BufferInputs)
                    // receive the inputs before we write them in the server entities' buffer
                    .before(InputSystemSet::Update),
            );
            app.add_systems(
                FixedPreUpdate,
                unified_receive_input_message::<P, A>
                    .in_set(InputSystemSet::ReceiveInputs)
                    .run_if(not(is_in_rollback)),
            );
        } else {
            // we don't want to add this plugin in HostServer mode because it's already added on the client side

            // NOTE: we need to add the leafwing server plugin because it ticks Action-States (so just-pressed become pressed)
            app.add_plugins(InputManagerPlugin::<A>::server());
        }
    }
}

/// For each entity that has an action-state, insert an action-state-buffer
/// that will store the value of the action-state for the last few ticks
/// (we use a buffer because the client's inputs might arrive out of order)
fn add_action_diff_buffer<A: LeafwingUserAction>(
    mut commands: Commands,
    action_state: Query<Entity, (Added<ActionState<A>>, Without<ActionDiffBuffer<A>>)>,
) {
    for entity in action_state.iter() {
        commands
            .entity(entity)
            .insert(ActionDiffBuffer::<A>::default());
    }
}

/// In unified mode, the server and client are running in the same app, so we can
/// read the inputs directly from the ActionDiffEvent events.
/// We read the diffs and update the ActionDiffBuffers
fn unified_receive_input_message<P: Protocol, A: LeafwingUserAction>(
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    diff_reader: Res<Events<ActionDiffEvent<A>>>,
    // used to check if the input entity was predicted, confirmed, or prepredicted
    input_entity_query: Query<(Option<&Predicted>, Option<&PrePredicted>), With<InputMap<A>>>,
    client_connection: Res<crate::client::connection::ConnectionManager<P>>,
    mut server_buffers: Query<&mut ActionDiffBuffer<A>, Without<InputMap<A>>>,
    server_connection: Res<ConnectionManager<P>>,
    client_net_connection: Res<crate::connection::client::ClientConnection>,
) {
    // get the client's input delay
    let delay = config.prediction.input_delay_ticks as i16;
    let tick = tick_manager.tick() + delay;
    // warn!("in read unified diffs");
    // warn!(?diff_reader, "read unified diffs");
    for diff in diff_reader.get_reader().read(&diff_reader) {
        // warn!(?tick, "received action diff: {diff:?}");
        if let Some(entity) = diff.owner {
            // map the client entity to the correct server entity
            if let Ok((predicted, pre_predicted)) = input_entity_query.get(entity) {
                if let Some(server_entity) = pre_predicted
                    .and_then(|pre_predicted| {
                        // 0. if the entity is pre-predicted, we need to map it using the server's input map
                        server_connection
                            .connection(client_net_connection.id())
                            .ok()
                            .and_then(|connection| {
                                trace!("found server entity for pre-predicted entity: {entity:?}");
                                connection
                                    .replication_receiver
                                    .remote_entity_map
                                    .get_local(entity)
                                    .copied()
                            })
                    })
                    .or_else(|| {
                        // 1. if the entity is confirmed, we need to convert the entity to the server's entity using the client's remote entity map
                        // 2. if the entity is predicted, we need to first convert the entity to confirmed, and then from confirmed to remote
                        predicted
                            .map_or(Some(entity), |p| p.confirmed_entity)
                            .and_then(|confirmed| {
                                client_connection
                                    .replication_receiver
                                    .remote_entity_map
                                    .get_remote(confirmed)
                                    .copied()
                            })
                    })
                {
                    // write to the server entity's ActionDiffBuffer
                    if let Ok(mut buffer) = server_buffers.get_mut(server_entity) {
                        trace!("set action for entity: {server_entity:?}");
                        buffer.set(tick, diff.action_diff.clone());
                    }
                }
            }
        } else {
            error!("Leafwing inputs stored in Resources are not supported currently");
        }
    }
}

/// Read the input messages from the server events to update the ActionDiffBuffers
fn receive_input_message<P: Protocol, A: LeafwingUserAction>(
    // mut global: Option<ResMut<ActionDiffBuffer<A>>>,
    mut connection_manager: ResMut<ConnectionManager<P>>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    mut query: Query<&mut ActionDiffBuffer<A>>,
) where
    P::Message: TryInto<InputMessage<A>, Error = ()>,
{
    // let manager = &mut server.connection_manager;
    for (mut message, client_id) in connection_manager.events.into_iter_input_messages::<A>() {
        debug!(action = ?A::short_type_path(), ?message.end_tick, ?message.diffs, "received input message");

        for (target, diffs) in std::mem::take(&mut message.diffs) {
            match target {
                // for pre-predicted entities, we already did the mapping on server side upon receiving the message
                // for non-pre predicted entities, the mapping was already done on client side
                InputTarget::Entity(entity) | InputTarget::PrePredictedEntity(entity) => {
                    debug!("received input for entity: {:?}", entity);
                    if let Ok(mut buffer) = query.get_mut(entity) {
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
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::InputPlugin;
    use bevy::utils::Duration;
    use leafwing_input_manager::prelude::ActionState;

    use crate::inputs::leafwing::input_buffer::ActionDiff;
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
        stepper.client_app.add_plugins((
            crate::client::input_leafwing::LeafwingInputPlugin::<MyProtocol, LeafwingInput1>::new(
                LeafwingInputConfig {
                    // NOTE: for simplicity, we send every diff (because otherwise it's hard to send an input after the tick system)
                    send_diffs_only: false,
                    ..default()
                },
            ),
            InputPlugin,
        ));
        // let press_action_id = stepper.client_app.world.register_system(press_action);
        stepper.server_app.add_plugins((
            LeafwingInputPlugin::<MyProtocol, LeafwingInput1>::default(),
            InputPlugin,
        ));
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
            .resource::<ClientConnectionManager>()
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
        stepper.frame_step();
        // client tick when we send the Jump action
        let client_tick = stepper.client_tick();
        stepper
            .client_app
            .world
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        stepper.frame_step();

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
        stepper.frame_step();
        stepper.frame_step();
    }
}
