use std::marker::PhantomData;

use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::components::{ComponentSyncMode, SyncComponent, SyncMetadata};
use crate::client::interpolation::despawn::{despawn_interpolated, removed_components};
use crate::client::interpolation::interpolate::{
    insert_interpolated_component, interpolate, update_interpolate_status,
};
use crate::client::interpolation::resource::InterpolationManager;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::Protocol;

use super::interpolation_history::{
    add_component_history, apply_confirmed_update_mode_full, apply_confirmed_update_mode_simple,
};
use super::spawn_interpolated_entity;

// TODO: maybe this is not an enum and user can specify multiple values, and we use the max delay between all of them?
#[derive(Clone)]
pub struct InterpolationDelay {
    /// The minimum delay that we will apply for interpolation
    /// This should be big enough so that the interpolated entity always has a server snapshot
    /// to interpolate towards.
    /// Set to 0.0 if you want to only use the Ratio
    pub min_delay: Duration,
    /// The interpolation delay is a ratio of the update-rate from the server
    /// The higher the server update_rate (i.e. smaller send_interval), the smaller the interpolation delay
    /// Set to 0.0 if you want to only use the Delay
    pub send_interval_ratio: f32,
}

impl Default for InterpolationDelay {
    fn default() -> Self {
        Self {
            min_delay: Duration::from_millis(0),
            send_interval_ratio: 2.0,
        }
    }
}

impl InterpolationDelay {
    pub fn with_min_delay(mut self, min_delay: Duration) -> Self {
        self.min_delay = min_delay;
        self
    }

    pub fn with_send_interval_ratio(mut self, send_interval_ratio: f32) -> Self {
        self.send_interval_ratio = send_interval_ratio;
        self
    }

    /// How much behind the latest server update we want the interpolation time to be
    pub(crate) fn to_duration(&self, server_send_interval: Duration) -> Duration {
        // TODO: deal with server_send_interval = 0 (set to frame rate)
        let ratio_value = server_send_interval.mul_f32(self.send_interval_ratio);
        std::cmp::max(ratio_value, self.min_delay)
    }
}

/// Config to specify how the snapshot interpolation should behave
#[derive(Clone)]
pub struct InterpolationConfig {
    pub delay: InterpolationDelay,
    /// If true, disable the interpolation logic (but still keep the internal component history buffers)
    /// The user will have to manually implement
    pub custom_interpolation_logic: bool,
    // How long are we keeping the history of the confirmed entities so we can interpolate between them?
    // pub(crate) interpolation_buffer_size: Duration,
}

#[allow(clippy::derivable_impls)]
impl Default for InterpolationConfig {
    fn default() -> Self {
        Self {
            delay: InterpolationDelay::default(),
            custom_interpolation_logic: false,
            // interpolation_buffer_size: Duration::from_millis(100),
        }
    }
}

impl InterpolationConfig {
    pub fn with_delay(mut self, delay: InterpolationDelay) -> Self {
        self.delay = delay;
        self
    }
}

pub struct InterpolationPlugin<P: Protocol> {
    config: InterpolationConfig,

    // minimum_snapshots
    _marker: PhantomData<P>,
}

impl<P: Protocol> InterpolationPlugin<P> {
    pub(crate) fn new(config: InterpolationConfig) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }
}

impl<P: Protocol> Default for InterpolationPlugin<P> {
    fn default() -> Self {
        Self {
            config: InterpolationConfig::default(),
            _marker: PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InterpolationSet {
    // PreUpdate Sets
    /// Restore the correct component values
    RestoreVisualInterpolation,
    // FixedUpdate
    /// Update the previous/current component values used for visual interpolation
    UpdateVisualInterpolationState,
    // Update Sets,
    /// Spawn interpolation entities,
    SpawnInterpolation,
    SpawnInterpolationFlush,
    /// Add component history for all interpolated entities' interpolated components
    SpawnHistory,
    SpawnHistoryFlush,
    /// Set to handle interpolated/confirmed entities/components getting despawned
    Despawn,
    DespawnFlush,
    /// Update component history, interpolation status
    PrepareInterpolation,
    /// Interpolate between last 2 server states. Has to be overriden if
    /// `InterpolationConfig.custom_interpolation_logic` is set to true
    Interpolate,
    // PostUpdate sets
    /// Interpolate the visual state of the game with 1 tick of delay
    VisualInterpolation,
}

// We want to run prediction:
// - after we received network events (PreUpdate)
// - before we run physics FixedUpdate (to not have to redo-them)

// - a PROBLEM is that ideally we would like to rollback the physics simulation
//   up to the client tick before we just updated the time. Maybe that's not a problem.. but we do need to keep track of the ticks correctly
//  the tick we rollback to would not be the current client tick ?
pub fn add_prepare_interpolation_systems<C: SyncComponent, P: Protocol>(app: &mut App)
where
    P::Components: SyncMetadata<C>,
{
    // TODO: maybe run this in PostUpdate?
    // TODO: maybe create an overarching prediction set that contains all others?
    app.add_systems(
        Update,
        (
            add_component_history::<C, P>.in_set(InterpolationSet::SpawnHistory),
            removed_components::<C>.in_set(InterpolationSet::Despawn),
        ),
    );
    match P::Components::mode() {
        ComponentSyncMode::Full => {
            app.add_systems(
                Update,
                (
                    apply_confirmed_update_mode_full::<C, P>,
                    update_interpolate_status::<C, P>,
                    // TODO: that means we could insert the component twice, here and then in interpolate...
                    //  need to optimize this
                    insert_interpolated_component::<C, P>,
                )
                    .chain()
                    .in_set(InterpolationSet::PrepareInterpolation),
            );
        }
        ComponentSyncMode::Simple => {
            app.add_systems(
                Update,
                apply_confirmed_update_mode_simple::<C, P>
                    .in_set(InterpolationSet::PrepareInterpolation),
            );
        }
        _ => {}
    }
}

// We add the interpolate system in different function because we might not want to add them
// in case there is custom interpolation logic.
pub fn add_interpolation_systems<C: Component + Clone, P: Protocol>(app: &mut App)
where
    P::Components: SyncMetadata<C>,
{
    app.add_systems(
        Update,
        interpolate::<C, P>.in_set(InterpolationSet::Interpolate),
    );
}

impl<P: Protocol> Plugin for InterpolationPlugin<P> {
    fn build(&self, app: &mut App) {
        P::Components::add_prepare_interpolation_systems(app);
        if !self.config.custom_interpolation_logic {
            P::Components::add_interpolation_systems(app);
        }

        // RESOURCES
        app.init_resource::<InterpolationManager>();
        // SETS
        app.configure_sets(
            Update,
            (
                InterpolationSet::SpawnInterpolation,
                InterpolationSet::SpawnInterpolationFlush,
                InterpolationSet::SpawnHistory,
                InterpolationSet::SpawnHistoryFlush,
                InterpolationSet::Despawn,
                InterpolationSet::DespawnFlush,
                InterpolationSet::PrepareInterpolation,
                InterpolationSet::Interpolate,
            )
                .chain(),
        );
        // SYSTEMS
        app.add_systems(
            Update,
            (
                // TODO: we want to run these flushes only if something actually happened in the previous set!
                //  because running the flush-system is expensive (needs exclusive world access)
                //  check how I can do this in bevy
                apply_deferred.in_set(InterpolationSet::SpawnInterpolationFlush),
                apply_deferred.in_set(InterpolationSet::SpawnHistoryFlush),
                apply_deferred.in_set(InterpolationSet::DespawnFlush),
            ),
        );
        app.add_systems(
            Update,
            (
                spawn_interpolated_entity::<P>.in_set(InterpolationSet::SpawnInterpolation),
                despawn_interpolated.in_set(InterpolationSet::Despawn),
            ),
        );
    }
}
