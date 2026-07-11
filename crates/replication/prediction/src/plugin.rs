use super::rollback::{RollbackPlugin, RollbackSystems, prepare_rollback};
use crate::SyncComponent;
use crate::correction::{
    repair_frame_interpolation_history, update_frame_interpolation_post_rollback,
};
use crate::despawn::PredictionDisable;
use crate::diagnostics::PredictionDiagnosticsPlugin;
use crate::manager::PredictionManager;
use crate::predicted_history::{
    PredictionHistory, add_history_diff_receiver, add_prediction_history,
    apply_component_removal_predicted, handle_tick_event_history_diff_receiver,
    handle_tick_event_prediction_history, prune_history_diff_receiver,
    snap_to_confirmed_during_rollback, update_prediction_history,
};
use crate::registry::PredictionRegistry;
use crate::rollback::DisabledDuringRollback;
#[cfg(feature = "metrics")]
use alloc::format;
use bevy_app::FixedPreUpdate;
use bevy_app::prelude::*;
use bevy_ecs::component::Mutable;
use bevy_ecs::entity_disabling::DefaultQueryFilters;
use bevy_ecs::prelude::*;
use bevy_replicon::shared::replication::diff::Diffable as RepliconDiffable;
#[cfg(feature = "metrics")]
use bevy_utils::prelude::DebugName;
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::host::HostClient;
use lightyear_core::prelude::ConfirmedHistory;
use lightyear_replication::prelude::ReplicationSystems;

/// Plugin that installs client-side prediction systems.
///
/// The systems run for connected, non-host client entities with a
/// [`PredictionManager`] component. Add `PredictionManager` to the local client
/// entity to opt that client into prediction and rollback.
#[derive(Default)]
pub struct PredictionPlugin;

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

pub(crate) type PredictionFilter = (
    With<PredictionManager>,
    With<Client>,
    With<Connected>,
    Without<HostClient>,
);

// NOTE: we need to run the prediction systems even if we're not synced, because we want
//  our HistoryBuffer to contain values for components/resources that were updated before syncing
//  is done.
/// Returns true if the client is not a HostClient and is Connected
pub(crate) fn should_run(query: Query<(), PredictionFilter>) -> bool {
    query.single().is_ok()
}

/// Enable rollbacking a component even if the component is not networked
pub fn add_non_networked_rollback_systems<C: Component<Mutability = Mutable> + Clone>(
    app: &mut App,
) {
    app.world_mut().register_component::<PredictionHistory<C>>();
    app.world_mut().register_component::<ConfirmedHistory<C>>();
    app.add_observer(apply_component_removal_predicted::<C>);
    app.add_observer(add_prediction_history::<C>);
    // Without this observer, the component's `PredictionHistory<C>` buffer
    // would not get its tick values shifted on timeline-sync, so any
    // history entries accumulated pre-sync would point to stale
    // (pre-sync) ticks after the client clock jumps forward.
    //
    // Do not shift `ConfirmedHistory<C>` here: confirmed samples are resolved
    // through `ReplicationCheckpointMap` and are already in authoritative
    // server tick space. Shifting them can move an init-message seed into the
    // future and make rollback prefer stale state over later server updates.
    app.add_observer(handle_tick_event_prediction_history::<C>);
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
            format!(
                "prediction::rollbacks::causes::{}::missing_on_confirmed",
                DebugName::type_name::<C>()
            ),
            metrics::Unit::Count,
            "Component present in the prediction history but missing on the confirmed entity"
        );
        metrics::describe_counter!(
            format!(
                "prediction::rollbacks::causes::{}::value_mismatch",
                DebugName::type_name::<C>()
            ),
            metrics::Unit::Count,
            "Component present in the prediction history but with a different value than on the confirmed entity"
        );
        metrics::describe_counter!(
            format!(
                "prediction::rollbacks::causes::{}::missing_on_predicted",
                DebugName::type_name::<C>()
            ),
            metrics::Unit::Count,
            "Component present in the confirmed entity but missing in the prediction history"
        );
        metrics::describe_counter!(
            format!(
                "prediction::rollbacks::causes::{}::removed_on_predicted",
                DebugName::type_name::<C>()
            ),
            metrics::Unit::Count,
            "Component present in the confirmed entity but removed in the prediction history"
        );
    }
    // TODO: register type if C is reflect
    // app.register_type::<HistoryState<C>>();
    // app.register_type::<PredictionHistory<C>>();

    app.add_observer(apply_component_removal_predicted::<C>);
    app.add_observer(handle_tick_event_prediction_history::<C>);
    app.add_observer(add_prediction_history::<C>);

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
    app.add_observer(handle_tick_event_history_diff_receiver::<C>);
    app.add_systems(
        PreUpdate,
        prune_history_diff_receiver::<C>.in_set(RollbackSystems::Prepare),
    );
}

impl Plugin for PredictionPlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.init_resource::<PredictionRegistry>();

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
            PredictionSystems::SnapToConfirmed.in_set(PredictionSystems::All),
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

        // PostUpdate
        app.configure_sets(PostUpdate, PredictionSystems::All.run_if(should_run));

        // PLUGINS
        app.add_plugins((PredictionDiagnosticsPlugin::default(), RollbackPlugin));
    }
}
