use super::rollback::{CatchUpGated, RollbackPlugin, RollbackSystems, prepare_rollback};
use crate::correction::{
    repair_frame_interpolation_history, update_frame_interpolation_post_rollback,
};
use crate::despawn::{PredictionDisable, finalize_deterministic_despawns};
use crate::diagnostics::PredictionDiagnosticsPlugin;
use crate::manager::{LastConfirmedInput, PredictionManager};
use crate::predicted_history::{
    PredictionHistory, add_history_diff_receiver, add_prediction_history,
    apply_component_removal_predicted, backfill_confirmed_history_on_predicted,
    handle_local_timeline_shift_history_diff_receiver,
    handle_local_timeline_shift_prediction_history, prune_history_diff_receiver,
    snap_to_confirmed_during_rollback, update_prediction_history,
};
use crate::registry::PredictionRegistry;
use crate::rollback::DisabledDuringRollback;
use crate::{Predicted, SyncComponent};
use bevy_app::FixedPreUpdate;
use bevy_app::prelude::*;
use bevy_ecs::component::Mutable;
use bevy_ecs::entity_disabling::DefaultQueryFilters;
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::AppMarkerExt;
use bevy_replicon::shared::replication::diff::Diffable as RepliconDiffable;
use bevy_replicon::shared::replication::receive_markers::MarkerConfig;
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
use lightyear_core::prelude::{ConfirmedHistory, is_in_rollback};
use lightyear_replication::prelude::{PreSpawned, PredictedSend, ReplicationSystems};
#[cfg(test)]
use lightyear_replication::prespawn::PreSpawnedReceiver;

/// Plugin that installs client-side prediction systems.
///
/// The systems run when the application contains a [`PredictionManager`] resource and its cached
/// network topology is a conventional client or active P2P session. P2P candidates do not run the
/// pipeline: deterministic play and its rollback history begin at the agreed session start tick.
/// Prediction-history writers also require a synchronized timeline and therefore never record
/// values under pre-sync tick labels. Insert a [`PredictionManager`] resource to opt the
/// application into one global prediction and rollback pipeline. Host-client and server
/// topologies remain authoritative and do not run it.
#[derive(Default)]
pub struct PredictionPlugin;

/// Registers Replicon's prediction markers for the shared client/server protocol.
///
/// Add this before registering predicted components. Lightyear's high-level
/// client and server plugin groups add it automatically.
#[derive(Default)]
pub struct PredictionMarkerPlugin;

// Higher-priority markers win when more than one applies to an update.
// Prespawn and catch-up must preserve their specialized reconciliation
// semantics before the general prediction markers are considered. Prespawn is
// strongest because a locally predicted absence must not be materialized by a
// lower-priority initial-write path.
const PRESPAWNED_PRIORITY: usize = 120;
const CATCH_UP_GATED_PRIORITY: usize = 110;
const PREDICTED_SEND_PRIORITY: usize = 100;
const PREDICTED_PRIORITY: usize = 90;

impl Plugin for PredictionMarkerPlugin {
    fn build(&self, app: &mut App) {
        app.register_marker_with::<CatchUpGated>(MarkerConfig {
            priority: CATCH_UP_GATED_PRIORITY,
            need_history: true,
        });
        // A matched prespawn must preserve its locally predicted live value
        // even when PredictedSend is included in the same authoritative update.
        app.register_marker_with::<PreSpawned>(MarkerConfig {
            priority: PRESPAWNED_PRIORITY,
            need_history: true,
        });
        app.register_marker_with::<PredictedSend>(MarkerConfig {
            priority: PREDICTED_SEND_PRIORITY,
            need_history: true,
        });
        // Keep the receiver-local marker registered for users that opt an
        // already replicated entity into prediction manually.
        app.register_marker_with::<Predicted>(MarkerConfig {
            priority: PREDICTED_PRIORITY,
            need_history: true,
        });
    }
}

/// Initialize resources required by the global prediction pipeline.
///
/// `LastConfirmedInput` is deliberately not removed with `PredictionManager`: it also represents
/// useful global input state for deterministic simulations that do not enable prediction.
fn initialize_prediction_resources(
    _trigger: On<Insert, PredictionManager>,
    mut commands: Commands,
) {
    commands.init_resource::<LastConfirmedInput>();
}

#[deprecated(note = "Use PredictionSystems instead")]
pub type PredictionSet = PredictionSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PredictionSystems {
    // PreUpdate Sets
    /// System set encompassing the sets in [`RollbackSystems`]
    Rollback,

    // FixedPreUpdate Sets
    /// During rollback re-simulation, snap components to confirmed values if we have them.
    /// This runs at the start of each FixedUpdate tick during rollback.
    SnapToConfirmed,

    // FixedPostUpdate Sets
    /// Set to deal with predicted/confirmed entities getting despawned
    /// In practice, the entities aren't despawned but all their components are removed
    EntityDespawn,
    /// Update the client's predicted history; runs after each physics step in the FixedUpdate Schedule
    UpdateHistory,

    /// General set encompassing all other system sets
    All,
}

/// Returns true if this application opted into prediction and has a supported active topology.
pub(crate) fn should_run(
    manager: Option<Res<PredictionManager>>,
    metadata: Res<NetworkingMetadata>,
) -> bool {
    manager.is_some()
        && matches!(
            metadata.mode,
            NetworkTopology::Client(_) | NetworkTopology::P2P(_)
        )
}

/// Enable rollbacking a component even if the component is not networked
pub fn add_non_networked_rollback_systems<C: Component<Mutability = Mutable> + Clone>(
    app: &mut App,
) {
    app.world_mut().register_component::<PredictionHistory<C>>();
    app.world_mut().register_component::<ConfirmedHistory<C>>();
    app.add_observer(apply_component_removal_predicted::<C>);
    app.add_observer(add_prediction_history::<C>);
    // This also supports explicit resynchronization after prediction has begun. Do not shift
    // `ConfirmedHistory<C>` here: confirmed samples are resolved
    // through `ReplicationCheckpointMap` and are already in authoritative
    // server tick space. Shifting them can move authoritative state into the
    // future and make rollback prefer it over later server updates.
    app.add_observer(handle_local_timeline_shift_prediction_history::<C>);
    app.add_systems(
        PreUpdate,
        (
            prepare_rollback::<C>.in_set(RollbackSystems::Prepare),
            repair_frame_interpolation_history::<C>
                .in_set(RollbackSystems::EndRollback)
                .before(update_frame_interpolation_post_rollback)
                .before(super::rollback::end_rollback),
        ),
    );
    app.add_systems(
        FixedPostUpdate,
        update_prediction_history::<C>.in_set(PredictionSystems::UpdateHistory),
    );
}

/// Enables rollbacking a resource. As a rule of thumb, only use on resources
/// that are only modified by systems in the `FixedMain` schedule. This is
/// because rollbacks only run the `FixedMain` schedule. For example, the
/// `Time<Fixed>` resource is modified by
/// `bevy_time::fixed::run_fixed_main_schedule()` which is run outside of the
/// `FixedMain` schedule and so it should not be used in this function.
///
/// As a side note, the `Time<Fixed>` resource is already rollbacked internally
/// by lightyear so that it can be used accurately within systems within the
/// `FixedMain` schedule during a rollback.
#[deprecated(note = "use `app.resource::<R>().local_rollback()` instead")]
pub fn add_resource_rollback_systems<R: Resource<Mutability = Mutable> + Clone>(app: &mut App) {
    add_non_networked_rollback_systems::<R>(app);
}

pub(crate) fn add_prediction_systems<C: SyncComponent>(app: &mut App) {
    #[cfg(feature = "metrics")]
    {
        metrics::describe_counter!(
            "prediction/rollback/causes",
            metrics::Unit::Count,
            "Prediction rollback causes, labeled by component and cause"
        );
        metrics::describe_gauge!(
            "prediction/rollback/history_values",
            metrics::Unit::Count,
            "Number of retained prediction-history values, labeled by component"
        );
    }
    // TODO: register type if C is reflect
    // app.register_type::<HistoryState<C>>();
    // app.register_type::<PredictionHistory<C>>();

    app.add_observer(apply_component_removal_predicted::<C>);
    app.add_observer(handle_local_timeline_shift_prediction_history::<C>);
    app.add_observer(add_prediction_history::<C>);
    app.add_observer(backfill_confirmed_history_on_predicted::<C>);

    app.add_systems(
        PreUpdate,
        (
            // for SyncMode::Full, we need to check if we need to rollback.
            // TODO: for mode=simple/once, we still need to re-add the component if the entity ends up not being despawned!
            // check_rollback::<C>.in_set(PredictionSet::CheckRollback),
            prepare_rollback::<C>.in_set(RollbackSystems::Prepare),
            repair_frame_interpolation_history::<C>
                .in_set(RollbackSystems::EndRollback)
                .before(update_frame_interpolation_post_rollback)
                .before(super::rollback::end_rollback),
        ),
    );
    app.add_systems(
        FixedPreUpdate,
        // During rollback re-simulation, snap to confirmed values if we have them
        snap_to_confirmed_during_rollback::<C>.in_set(PredictionSystems::SnapToConfirmed),
    );
    app.add_systems(
        FixedPostUpdate,
        // we need to run this during fixed update to know accurately the history for each tick
        update_prediction_history::<C>.in_set(PredictionSystems::UpdateHistory),
    );
}

pub(crate) fn add_prediction_diff_systems<C: SyncComponent + RepliconDiffable>(app: &mut App) {
    app.add_observer(add_history_diff_receiver::<C>);
    app.add_observer(handle_local_timeline_shift_history_diff_receiver::<C>);
    app.add_systems(
        PreUpdate,
        prune_history_diff_receiver::<C>.in_set(RollbackSystems::Prepare),
    );
}

impl Plugin for PredictionPlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.init_resource::<PredictionRegistry>();
        app.add_observer(initialize_prediction_resources);
        // Observers are not retroactive, so also handle applications that inserted their manager
        // before adding this plugin.
        if app.world().contains_resource::<PredictionManager>() {
            app.init_resource::<LastConfirmedInput>();
        }

        // Custom entity disabling
        let rollback_disable_id = app
            .world_mut()
            .register_component::<DisabledDuringRollback>();
        let prediction_disable_id = app.world_mut().register_component::<PredictionDisable>();
        app.world_mut()
            .resource_mut::<DefaultQueryFilters>()
            .register_disabling_component(rollback_disable_id);
        app.world_mut()
            .resource_mut::<DefaultQueryFilters>()
            .register_disabling_component(prediction_disable_id);

        // PreUpdate systems:
        // 1. Receive confirmed entities, add Confirmed and Predicted components
        // 2. (in prediction_systems) add ComponentHistory
        // 3. (in prediction_systems) Check if we should do rollback, clear histories and snap prediction's history to server-state
        // 4. Potentially do rollback
        app.configure_sets(
            PreUpdate,
            (
                ReplicationSystems::Receive,
                PredictionSystems::Rollback.in_set(PredictionSystems::All),
            )
                .chain(),
        );
        app.configure_sets(PreUpdate, PredictionSystems::All.run_if(should_run));

        // FixedPreUpdate systems
        // - During rollback, snap components to confirmed values if we have them
        app.configure_sets(
            FixedPreUpdate,
            PredictionSystems::SnapToConfirmed
                .in_set(PredictionSystems::All)
                .run_if(is_in_rollback),
        );
        app.configure_sets(FixedPreUpdate, PredictionSystems::All.run_if(should_run));

        // FixedPostUpdate systems
        // 1. Update client tick (don't run in rollback)
        // 2. Run main physics/game fixed-update loop
        // 3. Increment rollback tick (only run in fallback)
        // 4. Update predicted history
        app.configure_sets(
            FixedPostUpdate,
            (
                PredictionSystems::EntityDespawn,
                // for prespawned entities that could be spawned during FixedUpdate, we want to add the history
                // right away to avoid rollbacks
                PredictionSystems::UpdateHistory,
            )
                .in_set(PredictionSystems::All)
                .chain(),
        );
        app.configure_sets(FixedPostUpdate, PredictionSystems::All.run_if(should_run));
        app.add_systems(
            FixedPostUpdate,
            finalize_deterministic_despawns.in_set(PredictionSystems::EntityDespawn),
        );

        // PostUpdate
        app.configure_sets(PostUpdate, PredictionSystems::All.run_if(should_run));

        // PLUGINS
        app.add_plugins((PredictionDiagnosticsPlugin::default(), RollbackPlugin));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::system::RunSystemOnce;
    use lightyear_connection::p2p::P2P;
    use lightyear_core::timeline::Rollback;

    #[test]
    fn prediction_manager_initializes_global_input_frontier() {
        let mut app = App::new();
        app.add_plugins(PredictionPlugin);
        let manager = PredictionManager {
            rollback_policy: crate::manager::RollbackPolicy {
                max_rollback_ticks: 17,
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!app.world().contains_resource::<LastConfirmedInput>());
        app.insert_resource(manager);
        app.world_mut().flush();

        assert_eq!(
            app.world()
                .resource::<PredictionManager>()
                .rollback_policy
                .max_rollback_ticks,
            17
        );
        assert!(app.world().contains_resource::<LastConfirmedInput>());
    }

    #[test]
    fn prediction_manager_inserted_before_plugin_initializes_global_input_frontier() {
        let mut app = App::new();
        app.insert_resource(PredictionManager::default());

        app.add_plugins(PredictionPlugin);

        assert!(app.world().contains_resource::<LastConfirmedInput>());
    }

    #[test]
    fn prespawn_state_is_application_global() {
        let mut app = App::new();
        app.add_plugins(PredictionPlugin);

        assert!(app.world().contains_resource::<PreSpawnedReceiver>());
    }

    #[derive(Resource, Default)]
    struct SnapRuns(u32);

    fn count_snap_runs(mut runs: ResMut<SnapRuns>) {
        runs.0 += 1;
    }

    #[test]
    fn snap_to_confirmed_set_only_runs_during_rollback() {
        let mut app = App::new();
        app.add_plugins(PredictionPlugin);
        app.insert_resource(PredictionManager::default());
        app.init_resource::<SnapRuns>();
        let client = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::Client(client);
        app.add_systems(
            FixedPreUpdate,
            count_snap_runs.in_set(PredictionSystems::SnapToConfirmed),
        );

        app.world_mut().run_schedule(FixedPreUpdate);
        assert_eq!(app.world().resource::<SnapRuns>().0, 0);

        app.insert_resource(Rollback::FromInputs);
        app.world_mut().run_schedule(FixedPreUpdate);
        assert_eq!(app.world().resource::<SnapRuns>().0, 1);
    }

    #[test]
    fn prediction_pipeline_excludes_authoritative_topologies() {
        let mut app = App::new();
        app.init_resource::<NetworkingMetadata>();
        app.insert_resource(PredictionManager::default());
        let client = app.world_mut().spawn_empty().id();
        let server = app.world_mut().spawn_empty().id();

        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::Client(client);
        assert!(app.world_mut().run_system_once(should_run).unwrap());

        app.world_mut().entity_mut(client).insert(P2P::Candidate);
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::Undefined;
        assert!(!app.world_mut().run_system_once(should_run).unwrap());

        app.world_mut().entity_mut(client).insert(P2P::Joined);
        app.world_mut().resource_mut::<NetworkingMetadata>().mode =
            NetworkTopology::P2P([client].into_iter().collect());
        assert!(app.world_mut().run_system_once(should_run).unwrap());

        app.world_mut().resource_mut::<NetworkingMetadata>().mode =
            NetworkTopology::HostClient { server, client };
        assert!(!app.world_mut().run_system_once(should_run).unwrap());

        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::Server(server);
        assert!(!app.world_mut().run_system_once(should_run).unwrap());
    }
}
