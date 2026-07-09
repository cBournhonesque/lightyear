use crate::automation::AutomationClientPlugin;
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
        app.add_plugins(AutomationClientPlugin);
        app.add_systems(
            Update,
            interpolate.in_set(InterpolationSystems::Interpolate),
        );
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystems::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, (movement, shared_tail_behaviour).chain());
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_controlled_spawn);
        app.add_observer(handle_interpolated_spawn);

        app.add_plugins(FrameInterpolationPlugin);
        app.add_systems(Update, crate::debug::client::debug_pre_visual_interpolation);
        app.add_systems(Last, crate::debug::client::debug_post_visual_interpolation);
    }
}

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn buffer_input(
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Res<ButtonInput<KeyCode>>,
    mut last_direction: Local<Option<Direction>>,
) {
    if let Ok(mut action_state) = query.single_mut() {
        let requested_direction =
            if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
                Some(Direction::Up)
            } else if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
                Some(Direction::Down)
            } else if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
                Some(Direction::Left)
            } else if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
                Some(Direction::Right)
            } else {
                None
            };

        if let Some(direction) = requested_direction {
            let current_direction = last_direction.unwrap_or(Direction::Up);
            if current_direction.is_opposite(direction) {
                action_state.0 = Inputs::Empty;
            } else {
                *last_direction = Some(direction);
                action_state.0 = Inputs::Direction(direction);
            }
        } else {
            // we always set the value, so that the server can distinguish between no inputs received
            // and no keys pressed
            action_state.0 = Inputs::Empty
        }
    }
}

// Apply local input only to predicted entities owned by this client.
//
// If this example predicted remote entities, ownership would need to be checked before movement.
fn movement(
    mut position_query: Query<(&mut PlayerPosition, &ActionState<Inputs>), With<Predicted>>,
) {
    for (position, input) in position_query.iter_mut() {
        shared_movement_behaviour(position, input);
    }
}

/// Prepare predicted player entities for visual interpolation and distinguish them visually.
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
        // add visual interpolation for the head position of the predicted entity
        // so that the position gets updated smoothly every frame
        // (updating it only during FixedUpdate might cause visual artifacts, see:
        //  https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/visual_interpolation.html)
        commands.entity(entity).insert(FrameInterpolate);
    }
}

/// Add the local input marker once ownership is known.
///
/// In host-client worlds `Predicted` and `Controlled` may be added in different orders, so local
/// input setup should follow `Controlled` instead of the predicted-spawn visual setup.
pub(crate) fn handle_controlled_spawn(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    players: Query<Option<&ControlledBy>, (With<PlayerId>, Without<InputMarker<Inputs>>)>,
    clients: Query<(), With<Client>>,
) {
    let entity = trigger.entity;
    let Ok(controlled_by) = players.get(entity) else {
        return;
    };
    if let Some(controlled_by) = controlled_by {
        if clients.get(controlled_by.owner).is_err() {
            return;
        }
    }
    commands
        .entity(entity)
        .insert(InputMarker::<Inputs>::default());
}

/// Lower the saturation on interpolated entities so they are visually distinct.
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

// Custom interpolation needs both the head position and tail points so the interpolated entity
// stays on the confirmed tail path.
//
// We should always interpolate between the confirmed history values that bracket the
// interpolation tick.
enum ConfirmedInterpolationWindow<'a, C> {
    Single {
        tick: Tick,
        value: &'a C,
    },
    Pair {
        start_tick: Tick,
        start: &'a C,
        end_tick: Tick,
        end: &'a C,
    },
}

fn confirmed_interpolation_window<C>(
    history: &ConfirmedHistory<C>,
    interpolation_tick: Tick,
) -> Option<ConfirmedInterpolationWindow<'_, C>> {
    let previous_index = (0..history.len())
        .take_while(|i| {
            history
                .get_nth_tick(*i)
                .is_some_and(|tick| tick <= interpolation_tick)
        })
        .last()?;

    let (start_tick, start_state) = history.get_nth_state(previous_index)?;
    let HistoryState::Updated(start) = start_state else {
        return None;
    };

    let Some((end_tick, HistoryState::Updated(end))) = history.get_nth_state(previous_index + 1)
    else {
        return Some(ConfirmedInterpolationWindow::Single {
            tick: start_tick,
            value: start,
        });
    };

    Some(ConfirmedInterpolationWindow::Pair {
        start_tick,
        start,
        end_tick,
        end,
    })
}

pub(crate) fn interpolate(
    timeline: Single<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    mut parent_query: Query<
        (&mut PlayerPosition, &ConfirmedHistory<PlayerPosition>),
        With<Interpolated>,
    >,
    mut tail_query: Query<
        (
            Entity,
            &PlayerParent,
            &TailLength,
            &mut TailPoints,
            &ConfirmedHistory<TailPoints>,
        ),
        With<Interpolated>,
    >,
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

        let Some(tail_window) = confirmed_interpolation_window(tail_history, interpolation_tick)
        else {
            continue;
        };

        let (start_tick, tail_start_value, end_tick, tail_end_value, pos_start, pos_end) =
            match tail_window {
                ConfirmedInterpolationWindow::Single { tick, value } => {
                    let Some(pos) = parent_history.get_present(tick) else {
                        // the parent component has not been confirmed yet, so we can't interpolate
                        continue;
                    };
                    *tail = value.clone();
                    *parent_position = pos.clone();
                    continue;
                }
                ConfirmedInterpolationWindow::Pair {
                    start_tick,
                    start,
                    end_tick,
                    end,
                } => {
                    let Some(pos_start) = parent_history.get_present(start_tick) else {
                        // the parent component has not been confirmed yet, so we can't interpolate
                        continue;
                    };
                    let Some(pos_end) = parent_history.get_present(end_tick) else {
                        // the parent component has not been confirmed yet, so we can't interpolate
                        continue;
                    };
                    (start_tick, start, end_tick, end, pos_start, pos_end)
                }
            };

        // the new tail will be similar to the old tail, with some added points at the front
        *tail = tail_start_value.clone();
        *parent_position = pos_start.clone();

        // interpolation ratio
        let t = interpolation_fraction(
            start_tick,
            end_tick,
            interpolation_tick,
            interpolation_overstep,
        )
        .clamp(0.0, 1.0);
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
        if let Some(ratio) = pos_start.is_between(tail_end_value.0.front().unwrap().0, pos_end.0) {
            // we might need to add a new point to the tail
            if tail_end_value.0.front().unwrap().0 != tail_start_value.0.front().unwrap().0 {
                tail.0.push_front(tail_end_value.0.front().unwrap().clone());
                debug!("ADD POINT");
            }
            // the path is straight! just move the head and adjust the tail
            *parent_position = Ease::interpolating_curve_unbounded(*pos_start, *pos_end)
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
        tail_diff_length += segment_length(pos_end.0, tail_end_value.0.front().unwrap().0);

        // amount of distance we need to move the player by, while remaining on the path
        let mut pos_distance_to_do = 0.0;
        // segment [segment_idx-1, segment_idx] is the segment where the starting pos is.
        let mut segment_idx = 0;
        // else, keep trying to find in the remaining segments
        for i in 1..tail_end_value.0.len() {
            let segment_length = segment_length(tail_end_value.0[i].0, tail_end_value.0[i - 1].0);
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
