use super::interpolation_history::{
    add_component_history, apply_confirmed_update_mode_full, apply_confirmed_update_mode_simple,
};
use crate::despawn::{despawn_interpolated, removed_components};
use crate::interpolate::{insert_interpolated_component, interpolate, update_interpolate_status};
use crate::prelude::InterpolationRegistrationExt;
use crate::registry::InterpolationRegistry;
use crate::spawn::spawn_interpolated_entity;
use crate::timeline::TimelinePlugin;
use crate::{interpolated_on_add_hook, interpolated_on_remove_hook, Interpolated, InterpolationMode, SyncComponent};
use bevy::prelude::*;
use core::time::Duration;
use lightyear_connection::host::HostClient;
use lightyear_core::prelude::Tick;
use lightyear_core::time::PositiveTickDelta;
use lightyear_replication::control::Controlled;
use lightyear_replication::prelude::{AppComponentExt, ChildOfSync};
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_sync::plugin::SyncSet;
use serde::{Deserialize, Serialize};

/// Interpolation delay of the client at the time the message is sent
///
/// This component will be stored on the Client entities on the server
/// as an estimate of the interpolation delay of the client, for lag compensation.
#[derive(Serialize, Deserialize, Component, Default, Clone, Copy, PartialEq, Debug, Reflect)]
pub struct InterpolationDelay {
    /// Delay between the prediction time and the interpolation time
    pub delay: PositiveTickDelta,
}

impl InterpolationDelay {
    /// Get the tick and the overstep of the interpolation time by removing the delay
    /// from the current tick
    pub fn tick_and_overstep(&self, tick: Tick) -> (Tick, f32) {
        if self.delay.overstep.value() == 0.0 {
            (tick - self.delay.tick_diff, 0.0)
        } else {
            (
                tick - self.delay.tick_diff - 1,
                1.0 - self.delay.overstep.value(),
            )
        }
    }
}

impl ToBytes for InterpolationDelay {
    fn bytes_len(&self) -> usize {
        self.delay.bytes_len()
    }

    fn to_bytes(
        &self,
        buffer: &mut impl WriteInteger,
    ) -> core::result::Result<(), SerializationError> {
        self.delay.to_bytes(buffer)
    }

    fn from_bytes(buffer: &mut Reader) -> core::result::Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let delay = PositiveTickDelta::from_bytes(buffer)?;
        Ok(Self { delay })
    }
}

/// Config to specify how the snapshot interpolation should behave
#[derive(Clone, Copy, Reflect)]
pub struct InterpolationConfig {
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

impl Default for InterpolationConfig {
    fn default() -> Self {
        Self {
            min_delay: Duration::from_millis(5),
            send_interval_ratio: 1.3,
        }
    }
}

impl InterpolationConfig {
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
        core::cmp::max(ratio_value, self.min_delay)
    }
}

#[derive(Default)]
pub struct InterpolationPlugin {
    config: InterpolationConfig,
}

impl InterpolationPlugin {
    pub fn new(config: InterpolationConfig) -> Self {
        Self { config }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InterpolationSet {
    // Update Sets,
    /// Spawn interpolation entities,
    Spawn,
    /// Add component history for all interpolated entities' interpolated components
    SpawnHistory,
    /// Update component history, interpolation status
    Prepare,
    /// Interpolate between last 2 server states. Has to be overriden if
    /// `InterpolationConfig.custom_interpolation_logic` is set to true
    Interpolate,

    /// SystemSet encompassing all other interpolation sets
    All,
}

/// Add per-component systems related to interpolation
pub fn add_prepare_interpolation_systems<C: SyncComponent>(
    app: &mut App,
    interpolation_mode: InterpolationMode,
) {
    // TODO: maybe run this in PostUpdate?
    // TODO: maybe create an overarching prediction set that contains all others?
    app.add_systems(
        Update,
        add_component_history::<C>.in_set(InterpolationSet::SpawnHistory),
    );
    app.add_observer(removed_components::<C>);
    match interpolation_mode {
        InterpolationMode::Full => {
            app.add_systems(
                Update,
                (
                    apply_confirmed_update_mode_full::<C>,
                    update_interpolate_status::<C>,
                    // TODO: that means we could insert the component twice, here and then in interpolate...
                    //  need to optimize this
                    insert_interpolated_component::<C>,
                )
                    .chain()
                    .in_set(InterpolationSet::Prepare),
            );
        }
        InterpolationMode::Simple => {
            app.add_systems(
                Update,
                apply_confirmed_update_mode_simple::<C>.in_set(InterpolationSet::Prepare),
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
        app.add_plugins(TimelinePlugin);

        // PROTOCOL
        app.register_component::<Controlled>()
            .add_interpolation(InterpolationMode::Once);
        app.register_component::<ChildOfSync>()
            .add_interpolation(InterpolationMode::Once);

        // HOOKS
        // TODO: add tests for these!
        app.world_mut().register_component_hooks::<Interpolated>()
            .on_add(interpolated_on_add_hook)
            .on_remove(interpolated_on_remove_hook);

        // Host-Clients have no interpolation delay
        app.register_required_components::<HostClient, InterpolationDelay>();

        // REFLECT
        app.register_type::<InterpolationConfig>()
            .register_type::<InterpolationDelay>()
            .register_type::<Interpolated>();

        // RESOURCES
        app.init_resource::<InterpolationRegistry>();

        // SETS
        app.configure_sets(
            Update,
            (
                InterpolationSet::Spawn,
                InterpolationSet::SpawnHistory,
                // PrepareInterpolation uses the sync values (which are used to compute interpolation)
                InterpolationSet::Prepare.after(SyncSet::Sync),
                InterpolationSet::Interpolate,
            )
                .in_set(InterpolationSet::All)
                .chain(),
        );
        app.configure_sets(Update, InterpolationSet::All);
        // SYSTEMS
        app.add_systems(
            Update,
            spawn_interpolated_entity.in_set(InterpolationSet::Spawn),
        );
        app.add_observer(despawn_interpolated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear_core::time::Overstep;

    #[test]
    fn test_interpolation_delay() {
        let delay = InterpolationDelay {
            delay: PositiveTickDelta {
                tick_diff: 2,
                overstep: Default::default(),
            },
        };
        assert_eq!(delay.tick_and_overstep(Tick(3)), (Tick(1), 0.0));

        let delay = InterpolationDelay {
            delay: PositiveTickDelta {
                tick_diff: 2,
                overstep: Overstep::new(0.4),
            },
        };
        assert_eq!(delay.tick_and_overstep(Tick(3)), (Tick(0), 0.6));
    }
}
