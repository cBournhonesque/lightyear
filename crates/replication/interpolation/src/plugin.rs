use crate::archetypes::InterpolationWorld;
use crate::despawn::configure_delayed_interpolated_despawn;
use crate::interpolate::{apply_interpolation, update_interpolation_history};
use crate::registry::InterpolationRegistry;
use crate::timeline::InterpolationTimeline;
use crate::timeline::TimelinePlugin;
use bevy_app::{App, Plugin, PreUpdate, Update};
use bevy_ecs::{
    component::Component,
    prelude::*,
    schedule::{ApplyDeferred, IntoScheduleConfigs, SystemSet},
};
use bevy_reflect::Reflect;
use bevy_replicon::shared::replication::storage::ReplicationStorage;
use lightyear_connection::host::HostClient;
use lightyear_core::prelude::{Interpolated, Tick};
use lightyear_core::time::PositiveTickDelta;
use lightyear_replication::ReplicationSystems;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_sync::plugin::SyncSystems;
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
        if self.delay.overstep().value().is_zero() {
            (tick - self.delay.tick_diff(), 0.0)
        } else {
            (
                tick - self.delay.tick_diff() - 1,
                1.0 - self.delay.overstep().to_f32(),
            )
        }
    }
}

impl ToBytes for InterpolationDelay {
    fn bytes_len(&self) -> usize {
        self.delay.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.delay.to_bytes(buffer)
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let delay = PositiveTickDelta::from_bytes(buffer)?;
        Ok(Self { delay })
    }
}

// TODO: if Interpolated is added on an existing entity, we need to swap all its existing interpolated components to Confirmed<C>

/// Plugin that enables interpolating between replicated updates received from the remote.
///
/// Each remote update will be stored in a buffer, and the component will smoothly interpolate between two consecutive remote updates.
#[derive(Default)]
pub struct InterpolationPlugin;

#[deprecated(note = "Use InterpolationSystems instead")]
pub type InterpolationSet = InterpolationSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InterpolationSystems {
    // PreUpdate Sets,
    /// Sync components from the confirmed to the interpolated entity, and insert the ConfirmedHistory
    Sync,

    // Update
    /// Reserved for systems that should run before interpolation preparation.
    ///
    /// Interpolated archetypes are cached by the combined prepare system, so
    /// Lightyear does not install a built-in system in this set.
    Cache,

    /// Update component histories and apply Lightyear-owned interpolation.
    ///
    /// This runs in two ordered phases. The first phase updates histories and
    /// applies pending component insertions/removals at the interpolation
    /// timeline. Deferred commands are flushed before the second phase writes
    /// interpolated values for rules that enabled the apply phase.
    ///
    /// This can be in Update since we use the confirmed.tick to add values to the history, which is independent
    /// from the local tick.
    Prepare,
    /// Run user interpolation systems after Lightyear has prepared histories.
    ///
    /// Use this set for custom interpolation rules registered with
    /// `InterpolationFns::history_only`.
    Interpolate,

    /// SystemSet encompassing all other interpolation sets
    All,
}

/// Backfills `ConfirmedHistory<C>` for registered interpolation rules when
/// `Interpolated` is added after the live replicated component already exists.
fn backfill_confirmed_histories_on_interpolated(
    trigger: On<Add, Interpolated>,
    interpolation_registry: Res<InterpolationRegistry>,
    mut commands: Commands,
) {
    let Some(archetype) = trigger.trigger().new_archetype else {
        return;
    };

    for (live_component_id, history_component_id, backfill) in
        interpolation_registry.confirmed_history_backfill_fns()
    {
        if archetype.contains(live_component_id) && !archetype.contains(history_component_id) {
            backfill(trigger.entity, &mut commands);
        }
    }
}

#[derive(Debug, Default, Resource)]
pub(crate) struct InterpolationUpdateSystemState {
    generation: usize,
    finalized: bool,
}

pub(crate) fn refresh_update_interpolation_system_if_finalized(app: &mut App) {
    let Some(generation) = ({
        let Some(mut state) = app
            .world_mut()
            .get_resource_mut::<InterpolationUpdateSystemState>()
        else {
            return;
        };
        state.finalized.then(|| {
            state.generation += 1;
            state.generation
        })
    }) else {
        return;
    };
    add_update_interpolation_system_with_generation(app, generation);
}

/// Installs the type-erased interpolation update system.
pub(crate) fn add_update_interpolation_system(app: &mut App) {
    app.init_resource::<InterpolationUpdateSystemState>();
    let generation = app
        .world()
        .get_resource::<InterpolationUpdateSystemState>()
        .map_or(0, |state| state.generation);
    add_update_interpolation_system_with_generation(app, generation);
}

fn add_update_interpolation_system_with_generation(app: &mut App, installed_generation: usize) {
    let update_history_system =
        move |interpolation_world: InterpolationWorld,
              clients: Query<&InterpolationTimeline, Without<Interpolated>>,
              interpolation_registry: Res<InterpolationRegistry>,
              update_system_state: Res<InterpolationUpdateSystemState>,
              checkpoints: Res<ReplicationCheckpointMap>,
              replication_storage: Option<ResMut<ReplicationStorage>>,
              commands: Commands| {
            if update_system_state.generation != installed_generation {
                return;
            }
            update_interpolation_history(
                interpolation_world,
                clients,
                interpolation_registry,
                checkpoints,
                replication_storage,
                commands,
            );
        };

    let apply_system =
        move |interpolation_world: InterpolationWorld,
              clients: Query<&InterpolationTimeline, Without<Interpolated>>,
              interpolation_registry: Res<InterpolationRegistry>,
              update_system_state: Res<InterpolationUpdateSystemState>| {
            if update_system_state.generation != installed_generation {
                return;
            }
            apply_interpolation(interpolation_world, clients, interpolation_registry);
        };

    app.add_systems(
        Update,
        (update_history_system, ApplyDeferred, apply_system)
            .chain()
            .in_set(InterpolationSystems::Prepare),
    );
}

impl Plugin for InterpolationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TimelinePlugin);

        // RESOURCES
        app.init_resource::<InterpolationRegistry>();
        app.init_resource::<InterpolationUpdateSystemState>();
        configure_delayed_interpolated_despawn(app);
        app.add_observer(backfill_confirmed_histories_on_interpolated);

        // Host-Clients have no interpolation delay
        app.register_required_components::<HostClient, InterpolationDelay>();

        // SETS
        app.configure_sets(
            PreUpdate,
            InterpolationSystems::Sync
                .in_set(InterpolationSystems::All)
                .chain()
                .after(ReplicationSystems::Receive),
        );
        app.configure_sets(
            Update,
            (
                // PrepareInterpolation uses the sync values (which are used to compute interpolation)
                InterpolationSystems::Cache.after(SyncSystems::Sync),
                InterpolationSystems::Prepare,
                InterpolationSystems::Interpolate,
            )
                .in_set(InterpolationSystems::All)
                .chain(),
        );
        add_update_interpolation_system(app);
    }

    fn finish(&self, app: &mut App) {
        let world = app.world_mut();
        world
            .resource_mut::<InterpolationUpdateSystemState>()
            .finalized = true;
        world.resource_mut::<InterpolationRegistry>().finalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interpolation_delay() {
        let delay = InterpolationDelay {
            delay: PositiveTickDelta::lit("2"),
        };
        assert_eq!(delay.tick_and_overstep(Tick(3)), (Tick(1), 0.0));

        let delay = InterpolationDelay {
            delay: PositiveTickDelta::lit("2.4"),
        };
        let (tick, overstep) = delay.tick_and_overstep(Tick(3));
        assert_eq!(tick, Tick(0));
        assert!((overstep - 0.6).abs() < 0.0001);
    }
}
