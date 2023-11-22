use std::marker::PhantomData;
use std::time::Duration;

use crate::client::interpolation::despawn::{despawn_interpolated, removed_components};
use crate::client::interpolation::interpolate::{interpolate, update_interpolate_status};
use crate::plugin::sets::{FixedUpdateSet, MainSet};
use crate::{ComponentProtocol, Protocol};
use bevy::prelude::{
    apply_deferred, App, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PreUpdate,
    Res, SystemSet,
};

use super::interpolation_history::{add_component_history, update_component_history};
use super::{spawn_interpolated_entity, InterpolatedComponent, InterpolatedComponentMode};

// TODO: maybe this is not an enum and user can specify multiple values, and we use the max delay between all of them?
#[derive(Clone)]
pub enum InterpolationDelay {
    /// How much behind the client time the interpolated entities are
    /// This should be big enough that even if one server packet is loss
    Delay(Duration),
    /// How much behind the client entity the interpolated entity is in terms of ticks
    Ticks(u16),
    // /// The interpolation delay is a ratio of the update-rate from the server
    // /// Currently the server sends updates every frame, so the delay will be a ratio of the frame-rate
    // Ratio(f32),
}

impl Default for InterpolationDelay {
    fn default() -> Self {
        Self::Delay(Duration::from_millis(100))
    }
}

impl InterpolationDelay {
    // TODO: figure out how to not need to pass the arguments if we don't need them
    /// Compute how many ticks the interpolated entity is behind compared to the current entity
    pub(crate) fn tick_delta(
        &self,
        tick_duration: Duration,
        // server_update_rate: Duration,
    ) -> u16 {
        match self {
            InterpolationDelay::Delay(delay) => {
                (delay.as_secs_f64() / tick_duration.as_secs_f64()).ceil() as u16
            }
            InterpolationDelay::Ticks(ticks) => *ticks,
            // InterpolationDelay::Ratio(ratio) => (server_update_rate.mul_f32(*ratio).as_secs_f64()
            //     / tick_duration.as_secs_f64())
            // .ceil() as usize,
        }
    }
}

/// How much behind the client time the interpolated entities are
/// This will be converted to a tick
/// This should be

#[derive(Clone)]
pub struct InterpolationConfig {
    /// How much behind the client time the interpolated entities are
    /// This will be converted to a tick
    /// This should be
    pub(crate) delay: InterpolationDelay,
    /// How long are we keeping the history of the confirmed entities so we can interpolate between them?
    interpolation_buffer_size: Duration,
}

impl Default for InterpolationConfig {
    fn default() -> Self {
        Self {
            delay: InterpolationDelay::default(),
            // TODO: change
            interpolation_buffer_size: Duration::from_millis(100),
        }
    }
}

pub struct InterpolationPlugin<P: Protocol> {
    config: InterpolationConfig,

    // minimum_snapshots
    _marker: PhantomData<P>,
}

impl<P: Protocol> Default for InterpolationPlugin<P> {
    fn default() -> Self {
        Self {
            config: InterpolationConfig::default(),
            _marker: PhantomData::default(),
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InterpolationSet {
    // PreUpdate Sets
    // // Contains the other pre-update prediction sets
    // PreUpdateInterpolation,
    /// Spawn interpolation entities,
    SpawnInterpolation,
    SpawnInterpolationFlush,
    /// Add component history for all interpolated entities' interpolated components
    SpawnHistory,
    SpawnHistoryFlush,
    /// Set to handle interpolated/confirmed entities/components getting despawned
    Despawn,
    DespawnFlush,
    /// Update component history, interpolation status, and interpolate between last 2 server states
    Interpolate,
}

// We want to run prediction:
// - after we received network events (PreUpdate)
// - before we run physics FixedUpdate (to not have to redo-them)

// - a PROBLEM is that ideally we would like to rollback the physics simulation
//   up to the client tick before we just updated the time. Maybe that's not a problem.. but we do need to keep track of the ticks correctly
//  the tick we rollback to would not be the current client tick ?

pub fn add_interpolation_systems<C: InterpolatedComponent, P: Protocol>(app: &mut App) {
    // TODO: maybe create an overarching prediction set that contains all others?
    app.add_systems(
        PreUpdate,
        (
            (add_component_history::<C, P>).in_set(InterpolationSet::SpawnHistory),
            (removed_components::<C>).in_set(InterpolationSet::Despawn),
            (
                update_component_history::<C, P>,
                update_interpolate_status::<C, P>,
                interpolate::<C>,
            )
                .chain()
                .in_set(InterpolationSet::Interpolate),
        ),
    );
}

impl<P: Protocol> Plugin for InterpolationPlugin<P> {
    fn build(&self, app: &mut App) {
        P::Components::add_interpolation_systems(app);

        // PreUpdate systems:
        // 1. Receive confirmed entities, add Confirmed and Predicted components
        app.configure_sets(
            PreUpdate,
            (
                MainSet::Receive,
                InterpolationSet::SpawnInterpolation,
                InterpolationSet::SpawnInterpolationFlush,
                InterpolationSet::SpawnHistory,
                InterpolationSet::SpawnHistoryFlush,
                InterpolationSet::Despawn,
                InterpolationSet::DespawnFlush,
                InterpolationSet::Interpolate,
            )
                .chain(),
        );
        app.add_systems(
            PreUpdate,
            (
                // TODO: we want to run this flushes only if something actually happened in the previous set!
                //  because running the flush-system is expensive (needs exclusive world access)
                //  check how I can do this in bevy
                apply_deferred.in_set(InterpolationSet::SpawnInterpolationFlush),
                apply_deferred.in_set(InterpolationSet::SpawnHistoryFlush),
                apply_deferred.in_set(InterpolationSet::DespawnFlush),
            ),
        );

        app.add_systems(
            PreUpdate,
            (
                spawn_interpolated_entity.in_set(InterpolationSet::SpawnInterpolation),
                despawn_interpolated.in_set(InterpolationSet::Despawn),
            ),
        );
    }
}
