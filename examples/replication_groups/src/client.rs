use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;

use lightyear::_reexport::LinearInterpolator;
use lightyear::connection::netcode::NetcodeServer;
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour, shared_tail_behaviour};
use crate::{shared, ClientTransports, SharedSettings};

pub struct ClientPluginGroup {
    lightyear: ClientPlugin<MyProtocol>,
}
impl ClientPluginGroup {
    pub(crate) fn new(net_config: NetConfig) -> ClientPluginGroup {
        let config = ClientConfig {
            shared: shared_config(),
            net: net_config,
            interpolation: InterpolationConfig {
                delay: InterpolationDelay::default().with_send_interval_ratio(2.0),
                // do not do linear interpolation per component, instead we provide our own interpolation logic
                custom_interpolation_logic: true,
            },
            ..default()
        };
        let plugin_config = PluginConfig::new(config, protocol());
        ClientPluginGroup {
            lightyear: ClientPlugin::new(plugin_config),
        }
    }
}

impl PluginGroup for ClientPluginGroup {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(self.lightyear)
            .add(ExampleClientPlugin)
            .add(shared::SharedPlugin)
    }
}

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(PreUpdate, handle_connection.after(MainSet::ReceiveFlush));
        // app.add_systems(
        //     PostUpdate,
        //     debug_interpolate
        //         .before(InterpolationSet::PrepareInterpolation)
        //         .after(InterpolationSet::DespawnFlush),
        // );
        // app.add_systems(
        //     PreUpdate,
        //     debug_prediction_pre_rollback
        //         .after(PredictionSet::SpawnHistoryFlush)
        //         .before(PredictionSet::CheckRollback),
        // );
        // app.add_systems(
        //     PreUpdate,
        //     debug_prediction_post_rollback
        //         .after(PredictionSet::CheckRollback)
        //         .before(PredictionSet::Rollback),
        // );
        app.add_systems(
            PostUpdate,
            interpolate.in_set(InterpolationSet::Interpolate),
        );
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystemSet::BufferInputs),
        );
        app.add_systems(FixedUpdate, (movement, shared_tail_behaviour).chain());
        app.add_systems(Update, (handle_predicted_spawn, handle_interpolated_spawn));

        // add visual interpolation for the predicted snake (which gets updated in the FixedUpdate schedule)
        // (updating it only during FixedUpdate might cause visual artifacts, see:
        //  https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/visual_interpolation.html)
        app.add_plugins(VisualInterpolationPlugin::<PlayerPosition, MyProtocol>::default());
        app.add_systems(Update, debug_pre_visual_interpolation);
        app.add_systems(Last, debug_post_visual_interpolation);
    }
}

// Startup system for the client
pub(crate) fn init(mut commands: Commands, mut client: ResMut<ClientConnection>) {
    commands.spawn(Camera2dBundle::default());

    let _ = client.connect();
}

pub(crate) fn handle_connection(mut commands: Commands, metadata: Res<GlobalMetadata>) {
    // the `GlobalMetadata` resource holds metadata related to the client
    // once the connection is established.
    if metadata.is_changed() {
        if let Some(client_id) = metadata.client_id {
            commands.spawn(TextBundle::from_section(
                format!("Client {}", client_id),
                TextStyle {
                    font_size: 30.0,
                    color: Color::WHITE,
                    ..default()
                },
            ));
        }
    }
}

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn buffer_input(
    tick_manager: Res<TickManager>,
    mut connection_manager: ResMut<ClientConnectionManager>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    let tick = tick_manager.tick();
    if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
        return connection_manager.add_input(Inputs::Direction(Direction::Up), tick);
    }
    if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
        return connection_manager.add_input(Inputs::Direction(Direction::Down), tick);
    }
    if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
        return connection_manager.add_input(Inputs::Direction(Direction::Left), tick);
    }
    if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
        return connection_manager.add_input(Inputs::Direction(Direction::Right), tick);
    }
    if keypress.pressed(KeyCode::Backspace) {
        // currently, inputs is an enum and we can only add one input per tick
        return connection_manager.add_input(Inputs::Delete, tick);
    }
    if keypress.pressed(KeyCode::Space) {
        return connection_manager.add_input(Inputs::Spawn, tick);
    }
    return connection_manager.add_input(Inputs::None, tick);
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
pub(crate) fn movement(
    // TODO: maybe make prediction mode a separate component!!!
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
) {
    if <Components as SyncMetadata<PlayerPosition>>::mode() != ComponentSyncMode::Full {
        return;
    }
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            for position in position_query.iter_mut() {
                shared_movement_behaviour(position, input);
            }
        }
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    mut commands: Commands,
    mut predicted_heads: Query<(Entity, &mut PlayerColor), Added<Predicted>>,
    predicted_tails: Query<Entity, (With<PlayerParent>, Added<Predicted>)>,
) {
    for (entity, mut color) in predicted_heads.iter_mut() {
        color.0.set_s(0.3);
        // add visual interpolation for the head position of the predited entity
        // so that the position gets updated smoothly every frame
        // (updating it only during FixedUpdate might cause visual artifacts, see:
        //  https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/visual_interpolation.html)
        commands
            .entity(entity)
            .insert(VisualInterpolateStatus::<PlayerPosition>::default());
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    mut interpolated: Query<&mut PlayerColor, Added<Interpolated>>,
) {
    for mut color in interpolated.iter_mut() {
        color.0.set_s(0.1);
    }
}

pub(crate) fn debug_prediction_pre_rollback(
    tick_manager: Res<TickManager>,
    parent_query: Query<&PredictionHistory<PlayerPosition>>,
    tail_query: Query<(&PlayerParent, &PredictionHistory<TailPoints>)>,
) {
    trace!(tick = ?tick_manager.tick(),
        "prediction pre rollback debug");
    for (parent, tail_history) in tail_query.iter() {
        let parent_history = parent_query
            .get(parent.0)
            .expect("Tail entity has no parent entity!");
        trace!(?parent_history, "parent");
        trace!(?tail_history, "tail");
    }
}

pub(crate) fn debug_prediction_post_rollback(
    tick_manager: Res<TickManager>,
    parent_query: Query<&PredictionHistory<PlayerPosition>>,
    tail_query: Query<(&PlayerParent, &PredictionHistory<TailPoints>)>,
) {
    trace!(tick = ?tick_manager.tick(), "prediction post rollback debug");
    for (parent, tail_history) in tail_query.iter() {
        let parent_history = parent_query
            .get(parent.0)
            .expect("Tail entity has no parent entity!");
        trace!(?parent_history, "parent");
        trace!(?tail_history, "tail");
    }
}

pub(crate) fn debug_pre_visual_interpolation(
    tick_manager: Res<TickManager>,
    query: Query<(&PlayerPosition, &VisualInterpolateStatus<PlayerPosition>)>,
) {
    let tick = tick_manager.tick();
    for (position, interpolate_status) in query.iter() {
        trace!(
            ?tick,
            ?position,
            ?interpolate_status,
            "pre visual interpolation"
        );
    }
}

pub(crate) fn debug_post_visual_interpolation(
    tick_manager: Res<TickManager>,
    query: Query<(&PlayerPosition, &VisualInterpolateStatus<PlayerPosition>)>,
) {
    let tick = tick_manager.tick();
    for (position, interpolate_status) in query.iter() {
        trace!(
            ?tick,
            ?position,
            ?interpolate_status,
            "post visual interpolation"
        );
    }
}

pub(crate) fn debug_interpolate(
    tick_manager: Res<TickManager>,
    parent_query: Query<(
        &InterpolateStatus<PlayerPosition>,
        &ConfirmedHistory<PlayerPosition>,
    )>,
    tail_query: Query<(
        &PlayerParent,
        &InterpolateStatus<TailPoints>,
        &ConfirmedHistory<TailPoints>,
    )>,
) {
    info!(tick = ?tick_manager.tick(), "interpolation debug");
    for (parent, tail_status, tail_history) in tail_query.iter() {
        let (parent_status, parent_history) = parent_query
            .get(parent.0)
            .expect("Tail entity has no parent entity!");
        info!(?parent_status, ?parent_history, "parent");
        info!(?tail_status, ?tail_history, "tail");
    }
}

// Here, we want to have a custom interpolation logic, because we need to query two components
// at once to do the interpolation correctly.
// We want the interpolated entity to stay on the tail path of the confirmed entity at all times.
// The `InterpolateStatus` provides the start and end tick + component value, making it easy to perform interpolation.
pub(crate) fn interpolate(
    mut parent_query: Query<(&mut PlayerPosition, &InterpolateStatus<PlayerPosition>)>,
    mut tail_query: Query<(
        &PlayerParent,
        &TailLength,
        &mut TailPoints,
        &InterpolateStatus<TailPoints>,
    )>,
) {
    'outer: for (parent, tail_length, mut tail, tail_status) in tail_query.iter_mut() {
        let (mut parent_position, parent_status) = parent_query
            .get_mut(parent.0)
            .expect("Tail entity has no parent entity!");
        info!(
            ?parent_position,
            ?tail,
            ?parent_status,
            ?tail_status,
            "interpolate situation"
        );
        // the ticks should be the same for both components
        if let Some((start_tick, tail_start_value)) = &tail_status.start {
            if parent_status.start.is_none() {
                // the parent component has not been confirmed yet, so we can't interpolate
                continue;
            }
            let pos_start = &parent_status.start.as_ref().unwrap().1;
            if let Some((end_tick, tail_end_value)) = &tail_status.end {
                if parent_status.end.is_none() {
                    // the parent component has not been confirmed yet, so we can't interpolate
                    continue;
                }
                let pos_end = &parent_status.end.as_ref().unwrap().1;
                if start_tick != end_tick {
                    // the new tail will be similar to the old tail, with some added points at the front
                    *tail = tail_start_value.clone();
                    *parent_position = pos_start.clone();

                    // interpolation ratio
                    let t = tail_status.interpolation_fraction().unwrap();
                    let mut tail_diff_length = 0.0;
                    // find in which end tail segment the previous head_position is

                    // deal with the first segment separately
                    if let Some(ratio) =
                        pos_start.is_between(tail_end_value.0.front().unwrap().0, pos_end.0)
                    {
                        // we might need to add a new point to the tail
                        if tail_end_value.0.front().unwrap().0
                            != tail_start_value.0.front().unwrap().0
                        {
                            tail.0.push_front(tail_end_value.0.front().unwrap().clone());
                            debug!("ADD POINT");
                        }
                        // the path is straight! just move the head and adjust the tail
                        *parent_position = LinearInterpolator::lerp(pos_start, pos_end, t);
                        tail.shorten_back(parent_position.0, tail_length.0);
                        debug!(
                            ?tail,
                            ?parent_position,
                            "after interpolation; FIRST SEGMENT"
                        );
                        continue;
                    }

                    // else, the final head position is not on the first segment
                    tail_diff_length +=
                        segment_length(pos_end.0, tail_end_value.0.front().unwrap().0);

                    // amount of distance we need to move the player by, while remaining on the path
                    let mut pos_distance_to_do = 0.0;
                    // segment [segment_idx-1, segment_idx] is the segment where the starting pos is.
                    let mut segment_idx = 0;
                    // else, keep trying to find in the remaining segments
                    for i in 1..tail_end_value.0.len() {
                        let segment_length =
                            segment_length(tail_end_value.0[i].0, tail_end_value.0[i - 1].0);
                        if let Some(ratio) =
                            pos_start.is_between(tail_end_value.0[i].0, tail_end_value.0[i - 1].0)
                        {
                            if ratio == 0.0 {
                                // need to add a new point
                                tail.0.push_front(tail_end_value.0[i].clone());
                            }
                            // we found the segment where the starting pos is.
                            // let's find the total amount that the tail moved
                            tail_diff_length += (1.0 - ratio) * segment_length;
                            pos_distance_to_do = t * tail_diff_length;
                            segment_idx = i;
                            break;
                        } else {
                            tail_diff_length += segment_length;
                        }
                    }

                    // now move the head by `pos_distance_to_do` while remaining on the tail path
                    for i in (0..segment_idx).rev() {
                        let dist = segment_length(parent_position.0, tail_end_value.0[i].0);
                        debug!(
                            ?i,
                            ?dist,
                            ?pos_distance_to_do,
                            ?tail_diff_length,
                            ?segment_idx,
                            "in other segments"
                        );
                        if pos_distance_to_do < 1000.0 * f32::EPSILON {
                            debug!(?tail, ?parent_position, "after interpolation; ON POINT");
                            // no need to change anything
                            continue 'outer;
                        }
                        // if (dist - pos_distance_to_do) < 1000.0 * f32::EPSILON {
                        //     // the head is on a point (do not add a new point yet)
                        //     parent_position.0 = tail_end_value.0[i].0;
                        //     pos_distance_to_do -= dist;
                        //     tail.shorten_back(parent_position.0, tail_length.0);
                        //     info!(?tail, ?parent_position, "after interpolation; ON POINT");
                        //     continue;
                        // } else if dist > pos_distance_to_do {
                        if dist < pos_distance_to_do {
                            // the head must go through this tail point
                            parent_position.0 = tail_end_value.0[i].0;
                            tail.0.push_front(tail_end_value.0[i]);
                            pos_distance_to_do -= dist;
                        } else {
                            // we found the final segment where the head will be
                            parent_position.0 = tail
                                .0
                                .front()
                                .unwrap()
                                .1
                                .get_tail(tail_end_value.0[i].0, dist - pos_distance_to_do);
                            tail.shorten_back(parent_position.0, tail_length.0);
                            debug!(?tail, ?parent_position, "after interpolation; ELSE");
                            continue 'outer;
                        }
                    }
                    // the final position is on the first segment
                    let dist = segment_length(pos_end.0, tail_end_value.0.front().unwrap().0);
                    parent_position.0 = tail
                        .0
                        .front()
                        .unwrap()
                        .1
                        .get_tail(pos_end.0, dist - pos_distance_to_do);
                    tail.shorten_back(parent_position.0, tail_length.0);
                    debug!(?tail, ?parent_position, "after interpolation; ELSE FIRST");
                }
            }
        }
    }
}
