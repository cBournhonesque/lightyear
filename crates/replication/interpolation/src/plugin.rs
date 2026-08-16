use crate::despawn::configure_delayed_interpolated_despawn;
use crate::interpolate::{apply_interpolation, update_interpolation_history};
use crate::registry::{InterpolationRegistry, finalize_interpolation_registry};
use crate::timeline::TimelinePlugin;
use alloc::vec::Vec;
use bevy_app::{App, Last, Plugin, Update};
use bevy_ecs::{
    component::Component,
    entity_disabling::Disabled,
    prelude::*,
    schedule::{IntoScheduleConfigs, SystemSet},
};
use bevy_reflect::Reflect;
use lightyear_connection::host::HostClient;
use lightyear_core::prelude::{Interpolated, InterpolationPending, Tick};
use lightyear_core::time::PositiveTickDelta;
use lightyear_replication::send::ReplicatedInterpolationStart;
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

/// Backfills histories when [`Interpolated`] is added, including client-local additions.
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

/// After app and engine response systems finish, disables newly replicated
/// entities whose interpolated component histories are not ready yet.
fn mark_replicated_interpolation_pending(
    replicated_starts: Query<
        (Entity, &ReplicatedInterpolationStart),
        (
            With<ReplicatedInterpolationStart>,
            Allow<Disabled>,
            Allow<InterpolationPending>,
        ),
    >,
    interpolation_registry: Res<InterpolationRegistry>,
    mut commands: Commands,
) {
    let history_component_ids = interpolation_registry
        .confirmed_history_backfill_fns()
        .map(|(_, history_component_id, _)| history_component_id)
        .collect::<Vec<_>>();
    let starts = replicated_starts
        .iter()
        .map(|(entity, start)| (entity, start.tick))
        .collect::<Vec<_>>();
    commands.queue(move |world: &mut World| {
        for (entity, spawn_tick) in starts {
            let Ok(mut entity) = world.get_entity_mut(entity) else {
                continue;
            };
            if history_component_ids
                .iter()
                .any(|component_id| entity.contains_id(*component_id))
            {
                entity.insert(InterpolationPending { spawn_tick });
            }
            entity.remove::<ReplicatedInterpolationStart>();
        }
    });
}

impl Plugin for InterpolationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TimelinePlugin);

        // RESOURCES
        app.init_resource::<InterpolationRegistry>();
        app.register_disabling_component::<InterpolationPending>();
        configure_delayed_interpolated_despawn(app);
        app.add_observer(backfill_confirmed_histories_on_interpolated);
        app.add_systems(Last, mark_replicated_interpolation_pending);

        // Host-Clients have no interpolation delay
        app.register_required_components::<HostClient, InterpolationDelay>();

        // SETS
        app.configure_sets(
            Update,
            (
                // PrepareInterpolation uses the sync values (which are used to compute interpolation)
                InterpolationSystems::Prepare.after(SyncSystems::Sync),
                InterpolationSystems::Interpolate,
            )
                .in_set(InterpolationSystems::All)
                .chain(),
        );
        app.add_systems(
            Update,
            (update_interpolation_history, apply_interpolation)
                .chain()
                .in_set(InterpolationSystems::Prepare),
        );
    }

    fn finish(&self, app: &mut App) {
        finalize_interpolation_registry(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::AppInterpolationExt;
    use crate::rules::InterpolationFns;
    use bevy_app::{PostUpdate, PreUpdate};
    use bevy_replicon::client::confirm_history::ConfirmHistory;
    use bevy_replicon::prelude::Remote;
    use bevy_replicon::prelude::RepliconTick;
    use lightyear_core::prelude::ConfirmedHistory;
    use lightyear_replication::ReplicationSystems;

    #[derive(Component, Clone, PartialEq)]
    struct PendingTestComponent;

    #[derive(Resource, Default)]
    struct ReceivedEntity(Option<Entity>);

    #[derive(Resource, Default)]
    struct InterpolatedAddObservation {
        count: usize,
        was_pending: bool,
    }

    #[derive(Component)]
    struct ObserverAddedComponent;

    #[derive(Resource, Default)]
    struct ObserverAddedComponentObservation {
        count: usize,
        query_matched: bool,
        was_pending: bool,
    }

    #[derive(Resource, Default)]
    struct ScheduleObservation {
        update_count: usize,
        post_update_count: usize,
        last_count: usize,
    }

    const SPAWN_TICK: Tick = Tick(10);

    fn register_pending_test_history(app: &mut App) {
        app.interpolate_with::<PendingTestComponent>(InterpolationFns::history_only());
    }

    fn receive_interpolated_entity(mut commands: Commands, mut received: ResMut<ReceivedEntity>) {
        if received.0.is_none() {
            received.0 = Some(
                commands
                    .spawn((
                        Interpolated,
                        ReplicatedInterpolationStart { tick: SPAWN_TICK },
                        Remote,
                        ConfirmedHistory::<PendingTestComponent>::default(),
                    ))
                    .id(),
            );
        }
    }

    fn observe_interpolated_add(
        trigger: On<Add, Interpolated>,
        pending: Query<Has<InterpolationPending>>,
        mut observation: ResMut<InterpolatedAddObservation>,
    ) {
        observation.count += 1;
        observation.was_pending = pending.get(trigger.entity).unwrap_or_default();
    }

    fn add_component_from_interpolated(trigger: On<Add, Interpolated>, mut commands: Commands) {
        commands
            .entity(trigger.entity)
            .insert(ObserverAddedComponent);
    }

    fn observe_added_component(
        trigger: On<Add, ObserverAddedComponent>,
        pending: Query<Has<InterpolationPending>>,
        mut observation: ResMut<ObserverAddedComponentObservation>,
    ) {
        observation.count += 1;
        if let Ok(was_pending) = pending.get(trigger.entity) {
            observation.query_matched = true;
            observation.was_pending = was_pending;
        }
    }

    fn observe_interpolated_in_update(
        query: Query<Entity, With<Interpolated>>,
        mut observation: ResMut<ScheduleObservation>,
    ) {
        observation.update_count = query.iter().count();
    }

    fn observe_interpolated_in_post_update(
        query: Query<Entity, With<Interpolated>>,
        mut observation: ResMut<ScheduleObservation>,
    ) {
        observation.post_update_count = query.iter().count();
    }

    fn observe_interpolated_after_pending(
        query: Query<Entity, With<Interpolated>>,
        mut observation: ResMut<ScheduleObservation>,
    ) {
        observation.last_count = query.iter().count();
    }

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

    #[test]
    fn interpolation_pending_is_added_after_receive_observers_run() {
        let mut app = App::new();
        app.register_disabling_component::<InterpolationPending>();
        app.init_resource::<InterpolationRegistry>();
        app.init_resource::<ReceivedEntity>();
        app.init_resource::<InterpolatedAddObservation>();
        app.init_resource::<ScheduleObservation>();
        register_pending_test_history(&mut app);
        app.add_observer(backfill_confirmed_histories_on_interpolated);
        app.add_observer(observe_interpolated_add);
        app.add_systems(
            PreUpdate,
            receive_interpolated_entity.in_set(ReplicationSystems::Receive),
        );
        app.add_systems(Update, observe_interpolated_in_update);
        app.add_systems(PostUpdate, observe_interpolated_in_post_update);
        app.add_systems(Last, mark_replicated_interpolation_pending);
        app.add_systems(
            Last,
            observe_interpolated_after_pending.after(mark_replicated_interpolation_pending),
        );

        app.update();

        let entity = app.world().resource::<ReceivedEntity>().0.unwrap();
        let observation = app.world().resource::<InterpolatedAddObservation>();
        assert_eq!(observation.count, 1);
        assert!(!observation.was_pending);
        let schedule_observation = app.world().resource::<ScheduleObservation>();
        assert_eq!(schedule_observation.update_count, 1);
        assert_eq!(schedule_observation.post_update_count, 1);
        assert_eq!(schedule_observation.last_count, 0);
        assert_eq!(
            app.world().entity(entity).get::<InterpolationPending>(),
            Some(&InterpolationPending {
                spawn_tick: SPAWN_TICK
            })
        );
        assert!(
            !app.world()
                .entity(entity)
                .contains::<ReplicatedInterpolationStart>()
        );

        let mut default_query = app
            .world_mut()
            .query_filtered::<Entity, With<Interpolated>>();
        assert_eq!(default_query.iter(app.world()).count(), 0);

        let mut pending_query = app
            .world_mut()
            .query_filtered::<Entity, (With<Interpolated>, Allow<InterpolationPending>)>();
        assert_eq!(pending_query.iter(app.world()).count(), 1);
    }

    #[test]
    fn interpolation_pending_is_added_after_chained_add_observers_run() {
        let mut app = App::new();
        app.register_disabling_component::<InterpolationPending>();
        app.init_resource::<InterpolationRegistry>();
        app.init_resource::<ReceivedEntity>();
        app.init_resource::<ObserverAddedComponentObservation>();
        register_pending_test_history(&mut app);
        app.add_observer(backfill_confirmed_histories_on_interpolated);
        app.add_observer(add_component_from_interpolated);
        app.add_observer(observe_added_component);
        app.add_systems(
            PreUpdate,
            receive_interpolated_entity.in_set(ReplicationSystems::Receive),
        );
        app.add_systems(Last, mark_replicated_interpolation_pending);

        app.update();

        let entity = app.world().resource::<ReceivedEntity>().0.unwrap();
        let observation = app.world().resource::<ObserverAddedComponentObservation>();
        assert_eq!(observation.count, 1);
        assert!(observation.query_matched);
        assert!(!observation.was_pending);
        assert!(
            app.world()
                .entity(entity)
                .contains::<ObserverAddedComponent>()
        );
        assert!(
            app.world()
                .entity(entity)
                .contains::<InterpolationPending>()
        );
    }

    #[test]
    fn historyless_replicated_interpolation_is_ready_immediately() {
        let mut app = App::new();
        app.register_disabling_component::<InterpolationPending>();
        app.init_resource::<InterpolationRegistry>();
        app.add_observer(backfill_confirmed_histories_on_interpolated);
        app.add_systems(Last, mark_replicated_interpolation_pending);

        let entity = app
            .world_mut()
            .spawn((
                Interpolated,
                ReplicatedInterpolationStart { tick: SPAWN_TICK },
            ))
            .id();

        app.update();

        assert!(
            !app.world()
                .entity(entity)
                .contains::<InterpolationPending>()
        );
        assert!(
            !app.world()
                .entity(entity)
                .contains::<ReplicatedInterpolationStart>()
        );

        let mut default_query = app
            .world_mut()
            .query_filtered::<Entity, With<Interpolated>>();
        assert_eq!(default_query.iter(app.world()).count(), 1);
    }

    #[test]
    fn interpolation_pending_is_not_added_when_client_marks_remote_entity_interpolated() {
        let mut app = App::new();
        app.register_disabling_component::<InterpolationPending>();
        app.init_resource::<InterpolationRegistry>();
        app.add_observer(backfill_confirmed_histories_on_interpolated);
        app.add_systems(Last, mark_replicated_interpolation_pending);

        let entity = app
            .world_mut()
            .spawn((
                Remote,
                ConfirmHistory::new(RepliconTick::new(1)),
                PendingTestComponent,
            ))
            .id();
        app.world_mut().entity_mut(entity).insert(Interpolated);

        app.update();

        assert!(
            !app.world()
                .entity(entity)
                .contains::<InterpolationPending>()
        );
    }
}
