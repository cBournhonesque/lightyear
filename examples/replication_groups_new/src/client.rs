use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{shared_movement_behaviour, shared_tail_behaviour};
use bevy::prelude::*;
use core::time::Duration;
use lightyear::client::input::InputSystemSet;
use lightyear::inputs::native::{ActionState, InputMarker};
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // app.add_systems(
        //     PostUpdate,
        //     debug_interpolate
        //         .before(InterpolationSet::PrepareInterpolation)
        //         .after(InterpolationSet::DespawnFlush),
        // );
        app.add_systems(
            PostUpdate,
            interpolate.in_set(InterpolationSet::Interpolate),
        );
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystemSet::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, (movement, shared_tail_behaviour).chain());
        app.add_systems(Update, (handle_predicted_spawn, handle_interpolated_spawn));

        // add visual interpolation for the predicted snake (which gets updated in the FixedUpdate schedule)
        // (updating it only during FixedUpdate might cause visual artifacts, see:
        //  https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/visual_interpolation.html)
        app.add_plugins(VisualInterpolationPlugin::<PlayerPosition>::default());
        app.add_systems(Update, debug_pre_visual_interpolation);
        app.add_systems(Last, debug_post_visual_interpolation);
    }
}

// Startup system for the client
pub(crate) fn init(mut commands: Commands) {
    commands.connect_client();
}

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn buffer_input(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    if let Ok(mut action_state) = query.get_single_mut() {
        if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
            action_state.value = Some(Inputs::Direction(Direction::Up));
        } else if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
            action_state.value = Some(Inputs::Direction(Direction::Down));
        } else if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
            action_state.value = Some(Inputs::Direction(Direction::Left));
        } else if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
            action_state.value = Some(Inputs::Direction(Direction::Right));
        } else {
            // make sure to set the ActionState to None if no keys are pressed
            // otherwise the previous tick's ActionState will be used!
            action_state.value = None;
        }
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn movement(
    mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>), With<Predicted>>,
) {
    for (position, input) in position_query.iter_mut() {
        if let Some(inputs) = &input.value {
            shared_movement_behaviour(position, inputs);
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
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        // add visual interpolation for the head position of the predicted entity
        // so that the position gets updated smoothly every frame
        // (updating it only during FixedUpdate might cause visual artifacts, see:
        //  https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/visual_interpolation.html)
        commands.entity(entity).insert((
            VisualInterpolateStatus::<PlayerPosition>::default(),
            InputMarker::<Inputs>::default(),
        ));
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    mut interpolated: Query<&mut PlayerColor, Added<Interpolated>>,
) {
    for mut color in interpolated.iter_mut() {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
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
    debug!(tick = ?tick_manager.tick(), "interpolation debug");
    for (parent, tail_status, tail_history) in tail_query.iter() {
        let (parent_status, parent_history) = parent_query
            .get(parent.0)
            .expect("Tail entity has no parent entity!");
        debug!(?parent_status, ?parent_history, "parent");
        debug!(?tail_status, ?tail_history, "tail");
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
        debug!(
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
                        *parent_position = Linear::lerp(pos_start, pos_end, t);
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
