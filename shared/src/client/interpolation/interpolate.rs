use bevy::prelude::{Component, Query, ResMut};
use tracing::{info, warn};

use crate::client::components::SyncComponent;
use crate::client::interpolation::interpolation_history::ConfirmedHistory;
use crate::client::interpolation::InterpolatedComponent;
use crate::tick::Tick;
use crate::{Client, Protocol};

// TODO: the inner fields are pub just for integration testing.
//  maybe put the test here?
// NOTE: there's not a strict need for this, it just makes the logic easier to follow
#[derive(Component, PartialEq, Debug)]
pub struct InterpolateStatus<C: SyncComponent> {
    /// start tick to interpolate from, along with value
    pub start: Option<(Tick, C)>,
    /// end tick to interpolate to, along with value
    pub end: Option<(Tick, C)>,
    /// current interpolation tick
    pub current: Tick,
}

/// At the end of each frame, interpolate the components between the last 2 confirmed server states
pub(crate) fn update_interpolate_status<C: SyncComponent, P: Protocol>(
    mut client: ResMut<Client<P>>,
    mut query: Query<(&mut C, &mut InterpolateStatus<C>, &mut ConfirmedHistory<C>)>,
) {
    if !client.is_synced() {
        return;
    }
    let current_interpolate_tick = client.interpolated_tick();
    for (mut component, mut status, mut history) in query.iter_mut() {
        let mut start = status.start.take();
        let mut end = status.end.take();

        // if the interpolation tick is beyond the previous end tick,
        // we need to replace start with end, and clear end
        if let Some((end_tick, ref end_value)) = end {
            if end_tick <= current_interpolate_tick {
                start = end.clone();
                // TODO: this clone should be avoidable
                *component = end_value.clone();
                end = None;
            }
        }

        // clear all values with a tick <= current_interpolate_tick, and get the last cleared value
        // (we need to call this even if status.start is set, because a new more recent server update could have been received)
        let new_start = history.pop_until_tick(current_interpolate_tick);
        if let Some((new_tick, _)) = new_start {
            if start.as_ref().map_or(true, |(tick, _)| *tick < new_tick) {
                start = new_start;
            }
        }

        // get the next value immediately > current_interpolate_tick, but without popping
        // (we need to call this even if status.end is set, because a new more recent server update could have been received)
        if let Some((new_tick, _)) = history.peek() {
            if end.as_ref().map_or(true, |(tick, _)| new_tick < *tick) {
                // only pop if we actually put the value in end
                end = history.pop();
            }
        }

        info!(?current_interpolate_tick,
            last_received_server_tick = ?client.latest_received_server_tick(),
            start_tick = ?start.as_ref().map(|(tick, _)| tick),
            end_tick = ?end.as_ref().map(|(tick, _) | tick),
            "update_interpolate_status");
        status.start = start;
        status.end = end;
        status.current = current_interpolate_tick;
        if status.start.is_none() {
            warn!("no lerp start tick");
        }
        if status.end.is_none() {
            // warn!("no lerp end tick: might want to increase the interpolation delay");
        }
    }
}

pub(crate) fn interpolate<C: InterpolatedComponent>(
    mut query: Query<(&mut C, &InterpolateStatus<C>)>,
) {
    for (mut component, status) in query.iter_mut() {
        match (&status.start, &status.end) {
            (Some((start_tick, start_value)), Some((end_tick, end_value))) => {
                info!(?start_tick, ?end_tick, "doing interpolation!");
                if start_tick != end_tick {
                    let t =
                        (status.current - *start_tick) as f32 / (*end_tick - *start_tick) as f32;
                    *component = C::lerp(start_value.clone(), end_value.clone(), t);
                } else {
                    *component = start_value.clone();
                }
            }
            _ => {}
        }
    }
}
