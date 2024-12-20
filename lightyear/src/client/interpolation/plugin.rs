use bevy::prelude::*;
use bevy::utils::Duration;

use crate::client::components::{ComponentSyncMode, SyncComponent};
use crate::client::interpolation::despawn::{despawn_interpolated, removed_components};
use crate::client::interpolation::interpolate::{
    insert_interpolated_component, interpolate, update_interpolate_status,
};
use crate::client::interpolation::resource::InterpolationManager;
use crate::client::interpolation::spawn::spawn_interpolated_entity;
use crate::client::interpolation::Interpolated;
use crate::client::run_conditions::is_synced;
use crate::prelude::is_host_server;

use super::interpolation_history::{
    add_component_history, apply_confirmed_update_mode_full, apply_confirmed_update_mode_simple,
};

// TODO: maybe this is not an enum and user can specify multiple values, and we use the max delay between all of them?
#[derive(Clone, Copy, Reflect)]
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
    pub(crate) fn to_duration(self, server_send_interval: Duration) -> Duration {
        // TODO: deal with server_send_interval = 0 (set to frame rate)
        let ratio_value = server_send_interval.mul_f32(self.send_interval_ratio);
        std::cmp::max(ratio_value, self.min_delay)
    }
}

/// Config to specify how the snapshot interpolation should behave
#[derive(Clone, Copy, Reflect)]
pub struct InterpolationConfig {
    pub delay: InterpolationDelay,
    // How long are we keeping the history of the confirmed entities so we can interpolate between them?
    // pub(crate) interpolation_buffer_size: Duration,
}

#[allow(clippy::derivable_impls)]
impl Default for InterpolationConfig {
    fn default() -> Self {
        Self {
            delay: InterpolationDelay::default(),
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

#[derive(Default)]
pub struct InterpolationPlugin {
    config: InterpolationConfig,
}

impl InterpolationPlugin {
    pub(crate) fn new(config: InterpolationConfig) -> Self {
        Self { config }
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
    /// Add component history for all interpolated entities' interpolated components
    SpawnHistory,
    /// Update component history, interpolation status
    PrepareInterpolation,
    /// Interpolate between last 2 server states. Has to be overriden if
    /// `InterpolationConfig.custom_interpolation_logic` is set to true
    Interpolate,
    // PostUpdate sets
    /// Interpolate the visual state of the game with 1 tick of delay
    VisualInterpolation,

    /// SystemSet encompassing all other interpolation sets
    All,
}

/// Add per-component systems related to interpolation
pub fn add_prepare_interpolation_systems<C: SyncComponent>(
    app: &mut App,
    interpolation_mode: ComponentSyncMode,
) {
    // TODO: maybe run this in PostUpdate?
    // TODO: maybe create an overarching prediction set that contains all others?
    app.add_systems(
        Update,
        add_component_history::<C>.in_set(InterpolationSet::SpawnHistory),
    );
    app.observe(removed_components::<C>);
    match interpolation_mode {
        ComponentSyncMode::Full => {
            app.add_systems(
                Update,
                (
                    apply_confirmed_update_mode_full::<C>,
                    update_interpolate_status::<C>.run_if(is_synced),
                    // TODO: that means we could insert the component twice, here and then in interpolate...
                    //  need to optimize this
                    insert_interpolated_component::<C>,
                )
                    .chain()
                    .in_set(InterpolationSet::PrepareInterpolation),
            );
        }
        ComponentSyncMode::Simple => {
            app.add_systems(
                Update,
                apply_confirmed_update_mode_simple::<C>
                    .in_set(InterpolationSet::PrepareInterpolation),
            );
        }
        _ => {}
    }
}

// We add the interpolate system in different function because we might not want to add them
// in case there is custom interpolation logic.
pub fn add_interpolation_systems<C: SyncComponent>(app: &mut App) {
    app.add_systems(
        Update,
        interpolate::<C>.in_set(InterpolationSet::Interpolate),
    );
}

impl Plugin for InterpolationPlugin {
    fn build(&self, app: &mut App) {
        let should_run_interpolation = not(is_host_server).and_then(is_synced);

        // REFLECT
        app.register_type::<InterpolationConfig>()
            .register_type::<InterpolationDelay>()
            .register_type::<Interpolated>();

        // RESOURCES
        app.init_resource::<InterpolationManager>();
        // SETS
        app.configure_sets(
            Update,
            (
                InterpolationSet::SpawnInterpolation,
                InterpolationSet::SpawnHistory,
                InterpolationSet::PrepareInterpolation,
                InterpolationSet::Interpolate,
            )
                .in_set(InterpolationSet::All)
                .chain(),
        );
        app.configure_sets(
            Update,
            InterpolationSet::All.run_if(should_run_interpolation),
        );
        // SYSTEMS
        app.add_systems(
            Update,
            spawn_interpolated_entity.in_set(InterpolationSet::SpawnInterpolation),
        );
        app.observe(despawn_interpolated);
    }
}
