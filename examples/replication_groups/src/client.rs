use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{shared_movement_behaviour, shared_tail_behaviour};
use bevy::prelude::*;
use lightyear::prelude::input::client::*;
use lightyear::prelude::input::native::*;
use lightyear::prelude::*;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // app.add_systems(
        //     PostUpdate,
        //     debug_interpolate
        //         .before(InterpolationSet::PrepareInterpolation)
        //         .after(InterpolationSet::DespawnFlush),
        // );
        app.add_systems(
            PostUpdate,
            interpolate.in_set(InterpolationSystems::Interpolate),
        );
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystems::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, (movement, shared_tail_behaviour).chain());
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);

        // add visual interpolation for the predicted snake (which gets updated in the FixedUpdate schedule)
        // (updating it only during FixedUpdate might cause visual artifacts, see:
        //  https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/visual_interpolation.html)
        app.add_plugins(FrameInterpolationPlugin::<PlayerPosition>::default());
        app.add_systems(Update, debug_pre_visual_interpolation);
        app.add_systems(Last, debug_post_visual_interpolation);
    }
}

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn buffer_input(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    if let Ok(mut action_state) = query.single_mut() {
        if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
            action_state.0 = Inputs::Direction(Direction::Up);
        } else if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
            action_state.0 = Inputs::Direction(Direction::Down);
        } else if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
            action_state.0 = Inputs::Direction(Direction::Left);
        } else if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
            action_state.0 = Inputs::Direction(Direction::Right);
        } else {
            // we always set the value, so that the server can distinguish between no inputs received
            // and no keys pressed
            action_state.0 = Inputs::Empty
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
        shared_movement_behaviour(position, input);
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut predicted: Query<&mut PlayerColor, With<Predicted>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if let Ok(mut color) = predicted.get_mut(entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        warn!("Add InputMarker to entity: {:?}", entity);
        // add visual interpolation for the head position of the predicted entity
        // so that the position gets updated smoothly every frame
        // (updating it only during FixedUpdate might cause visual artifacts, see:
        //  https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/visual_interpolation.html)
        commands.entity(entity).insert((
            FrameInterpolate::<PlayerPosition>::default(),
            InputMarker::<Inputs>::default(),
        ));
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    trigger: On<Add, PlayerColor>,
    mut interpolated: Query<&mut PlayerColor, With<Interpolated>>,
) {
    if let Ok(mut color) = interpolated.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

pub(crate) fn debug_pre_visual_interpolation(
    timeline: Res<LocalTimeline>,
    query: Query<(&PlayerPosition, &FrameInterpolate<PlayerPosition>)>,
) {
    let tick = timeline.tick();
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
    timeline: Res<LocalTimeline>,
    query: Query<(&PlayerPosition, &FrameInterpolate<PlayerPosition>)>,
) {
    let tick = timeline.tick();
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
    timeline: Res<LocalTimeline>,
    parent_query: Query<(&ConfirmedHistory<PlayerPosition>,)>,
    tail_query: Query<(&PlayerParent, &ConfirmedHistory<TailPoints>)>,
) {
    debug!(tick = ?timeline.tick(), "interpolation debug");
    for (parent, tail_history) in tail_query.iter() {
        let parent_history = parent_query
            .get(parent.0)
            .expect("Tail entity has no parent entity!");
        debug!(?parent_history, "parent");
        debug!(?tail_history, "tail");
    }
}

// Here, we want to have a custom interpolation logic, because we need to query two components
// at once to do the interpolation correctly.
// We want the interpolated entity to stay on the tail path of the confirmed entity at all times.
//
// We should always interpolate between the first 2 values of the ConfirmedHistory if the
// interpolation_tick is between them
pub(crate) fn interpolate(
    registry: Res<InterpolationRegistry>,
    timeline: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    mut parent_query: Query<(&mut PlayerPosition, &ConfirmedHistory<PlayerPosition>)>,
    mut tail_query: Query<(
        Entity,
        &PlayerParent,
        &TailLength,
        &mut TailPoints,
        &ConfirmedHistory<TailPoints>,
    )>,
) {
    let interpolation_tick = timeline.tick();
    let interpolation_overstep = timeline.overstep().to_f32();
    'outer: for (tail_entity, parent, tail_length, mut tail, tail_history) in tail_query.iter_mut()
    {
        let Ok((mut parent_position, parent_history)) = parent_query.get_mut(parent.0) else {
            // TODO: could this be due that we don't sync at the same time?
            error!("Tail entity {tail_entity:?} has no parent entity!");
            continue;
        };

        // the ticks should be the same for both components
        if let Some((start_tick, tail_start_value)) = tail_history.start() {
            // make sure to stop early if the interpolation_tick is too early
            if interpolation_tick < start_tick {
                continue;
            };
            let Some((_, pos_start)) = parent_history.start() else {
                // the parent component has not been confirmed yet, so we can't interpolate
                continue;
            };
            if let Some((end_tick, tail_end_value)) = tail_history.end() {
                let Some((_, pos_end)) = parent_history.end() else {
                    // the parent component has not been confirmed yet, so we can't interpolate
                    continue;
                };
                if start_tick != end_tick {
                    // the new tail will be similar to the old tail, with some added points at the front
                    *tail = tail_start_value.clone();
                    *parent_position = pos_start.clone();

                    // interpolation ratio
                    let t = interpolation_fraction(
                        start_tick,
                        end_tick,
                        interpolation_tick,
                        interpolation_overstep,
                    );
                    debug!(
                        ?start_tick,
                        ?end_tick,
                        ?interpolation_tick,
                        ?t,
                        ?parent_history,
                        ?tail_history,
                        "interpolate situation"
                    );
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
                        *parent_position =
                            Ease::interpolating_curve_unbounded(*pos_start, *pos_end)
                                .sample_unchecked(t)
                                .clone();
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
