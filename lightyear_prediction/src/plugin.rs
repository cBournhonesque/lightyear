use super::pre_prediction::PrePredictionPlugin;
use super::predicted_history::apply_confirmed_update;
use super::resource_history::{
    handle_tick_event_resource_history, update_resource_history, ResourceHistory,
};
use super::rollback::{
    prepare_rollback, prepare_rollback_non_networked, prepare_rollback_prespawn, prepare_rollback_resource,
    remove_prediction_disable, run_rollback, RollbackPlugin,
};
use super::spawn::spawn_predicted_entity;
use crate::correction::{
    get_corrected_state, restore_corrected_state, set_original_prediction_post_rollback,
};
use crate::despawn::{despawn_confirmed, PredictionDisable};
use crate::diagnostics::PredictionDiagnosticsPlugin;
use crate::manager::PredictionManager;
use crate::predicted_history::{
    add_prediction_history, add_sync_systems, apply_component_removal_confirmed,
    apply_component_removal_predicted, handle_tick_event_prediction_history,
    update_prediction_history,
};
use crate::prespawn::{PreSpawned, PreSpawnedPlugin};
use crate::registry::PredictionRegistry;
use crate::{predicted_on_add_hook, predicted_on_remove_hook, Predicted, PredictionMode, SyncComponent};
use bevy::ecs::component::Mutable;
use bevy::ecs::entity_disabling::DefaultQueryFilters;
use bevy::prelude::*;
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::host::HostClient;
use lightyear_core::timeline::Rollback;
use lightyear_replication::prelude::ReplicationSet;

/// Plugin that enables client-side prediction
#[derive(Default)]
pub struct PredictionPlugin;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PredictionSet {
    // PreUpdate Sets
    /// Spawn predicted entities,
    /// We will also use this do despawn predicted entities when confirmed entities are despawned
    SpawnPrediction,
    /// Sync components from the Confirmed entity to the Predicted entity, and potentially
    /// insert PredictedHistory components
    Sync,
    /// Restore the Correct value instead of the VisualCorrected value
    /// - we need this here because we want the correct value before the rollback check
    /// - we are also careful to add this set to FixedPreUpdate as well, so that if FixedUpdate
    ///   runs multiple times in a row, we still correctly reset the component to the Correct value
    ///   before running a Simulation step. It's ok to have a duplicate system because we use core::mem::take
    RestoreVisualCorrection,
    /// Check if rollback is needed
    CheckRollback,

    // ROLLBACK
    /// If any Predicted entity was marked as despawned, instead of despawning them we simply disabled the entity.
    /// If we do a rollback we want to restore those entities.
    RemoveDisable,
    /// Prepare rollback by snapping the current state to the confirmed state and clearing histories
    /// For pre-spawned entities, we just roll them back to their historical state.
    /// If they didn't exist in the rollback tick, despawn them
    PrepareRollback,
    /// Perform rollback
    Rollback,
    // NOTE: no need to add RollbackFlush because running a schedule (which we do for rollback) will flush all commands at the end of each run

    // FixedPreUpdate Sets
    // RestoreVisualCorrection

    // FixedPostUpdate Sets
    /// Set to deal with predicted/confirmed entities getting despawned
    /// In practice, the entities aren't despawned but all their components are removed
    EntityDespawn,
    /// Update the client's predicted history; runs after each physics step in the FixedUpdate Schedule
    UpdateHistory,
    /// Visually interpolate the predicted components to the corrected state
    VisualCorrection,

    /// General set encompassing all other system sets
    All,
}

/// Returns true if we are in rollback
pub fn is_in_rollback(query: Query<(), (With<PredictionManager>, With<Rollback>)>) -> bool {
    query.single().is_ok()
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
pub fn add_non_networked_rollback_systems<
    C: Component<Mutability = Mutable> + PartialEq + Clone,
>(
    app: &mut App,
) {
    app.add_observer(apply_component_removal_predicted::<C>);
    app.add_observer(add_prediction_history::<C>);
    app.add_systems(
        PreUpdate,
        (prepare_rollback_non_networked::<C>.in_set(PredictionSet::PrepareRollback),),
    );
    app.add_systems(
        FixedPostUpdate,
        update_prediction_history::<C>.in_set(PredictionSet::UpdateHistory),
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
pub fn add_resource_rollback_systems<R: Resource + Clone>(app: &mut App) {
    // TODO: add these registrations if the type is reflect
    // app.register_type::<HistoryState<R>>();
    // app.register_type::<ResourceHistory<R>>();
    app.insert_resource(ResourceHistory::<R>::default());
    app.add_observer(handle_tick_event_resource_history::<R>);
    app.add_systems(
        PreUpdate,
        prepare_rollback_resource::<R>.in_set(PredictionSet::PrepareRollback),
    );
    app.add_systems(
        FixedPostUpdate,
        update_resource_history::<R>.in_set(PredictionSet::UpdateHistory),
    );
}

pub fn add_prediction_systems<C: SyncComponent>(app: &mut App, prediction_mode: PredictionMode) {
    match prediction_mode {
        PredictionMode::Full => {
            #[cfg(feature = "metrics")]
            {
                metrics::describe_counter!(
                    format!(
                        "prediction::rollbacks::causes::{}::missing_on_confirmed",
                        core::any::type_name::<C>()
                    ),
                    metrics::Unit::Count,
                    "Component present in the prediction history but missing on the confirmed entity"
                );
                metrics::describe_counter!(
                    format!(
                        "prediction::rollbacks::causes::{}::value_mismatch",
                        core::any::type_name::<C>()
                    ),
                    metrics::Unit::Count,
                    "Component present in the prediction history but with a different value than on the confirmed entity"
                );
                metrics::describe_counter!(
                    format!(
                        "prediction::rollbacks::causes::{}::missing_on_predicted",
                        core::any::type_name::<C>()
                    ),
                    metrics::Unit::Count,
                    "Component present in the confirmed entity but missing in the prediction history"
                );
                metrics::describe_counter!(
                    format!(
                        "prediction::rollbacks::causes::{}::removed_on_predicted",
                        core::any::type_name::<C>()
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
                // restore to the corrected state (as the visual state might be interpolating
                // between the predicted and corrected state)
                restore_corrected_state::<C>.in_set(PredictionSet::RestoreVisualCorrection),
            );
            app.add_systems(
                PreUpdate,
                (
                    // for SyncMode::Full, we need to check if we need to rollback.
                    // TODO: for mode=simple/once, we still need to re-add the component if the entity ends up not being despawned!
                    // check_rollback::<C>.in_set(PredictionSet::CheckRollback),
                    (prepare_rollback::<C>, prepare_rollback_prespawn::<C>)
                        .in_set(PredictionSet::PrepareRollback),
                ),
            );
            // we want this to run every frame.
            // If we have a Correction and we have 2 consecutive frames without FixedUpdate running
            // the component would be set to the corrected state, instead of the original prediction!
            app.add_systems(
                RunFixedMainLoop,
                set_original_prediction_post_rollback::<C>
                    .in_set(RunFixedMainLoopSystem::AfterFixedMainLoop),
            );
            // we need this in case the FixedUpdate schedule runs multiple times in a row.
            // Otherwise we would have
            // [PreUpdate] RestoreCorrectValue
            // [FixedUpdate] Step -> Correction = UpdateCorrectValue, InterpolateVisualValue, SetC=Visual, Sync, UpdateVisualInterpolation
            // [FixedUpdate] Step (from C=Visual!!)
            // We still need the RestoreVisualCorrection in PreUpdate because we need the correct state when checking for rollbacks
            // Maybe the rollback systems should be in FixedUpdate?
            app.add_systems(
                FixedPreUpdate,
                // restore to the corrected state (as the visual state might be interpolating
                // between the predicted and corrected state)
                restore_corrected_state::<C>.in_set(PredictionSet::RestoreVisualCorrection),
            );
            app.add_systems(
                FixedPostUpdate,
                (
                    get_corrected_state::<C>.in_set(PredictionSet::VisualCorrection),
                    // we need to run this during fixed update to know accurately the history for each tick
                    update_prediction_history::<C>.in_set(PredictionSet::UpdateHistory),
                ),
            );
        }
        PredictionMode::Simple => {
            app.add_observer(apply_component_removal_confirmed::<C>);
            app.add_systems(
                PreUpdate,
                (
                    // for SyncMode::Simple, just copy the confirmed components
                    apply_confirmed_update::<C>.in_set(PredictionSet::CheckRollback),
                ),
            );
        }
        _ => {}
    };
}

impl Plugin for PredictionPlugin {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<Predicted>()
            .register_type::<PreSpawned>()
            .register_type::<PredictionDisable>();
        
        // HOOKS
        app.world_mut().register_component_hooks::<Predicted>()
            .on_add(predicted_on_add_hook)
            .on_remove(predicted_on_remove_hook);

        // RESOURCES
        app.init_resource::<PredictionRegistry>();

        // Custom entity disabling
        let prediction_disable_id = app.world_mut().register_component::<PredictionDisable>();
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
                ReplicationSet::Receive,
                (
                    PredictionSet::SpawnPrediction,
                    PredictionSet::Sync,
                    PredictionSet::RestoreVisualCorrection,
                    PredictionSet::CheckRollback,
                    PredictionSet::RemoveDisable.run_if(is_in_rollback),
                    PredictionSet::PrepareRollback.run_if(is_in_rollback),
                    PredictionSet::Rollback.run_if(is_in_rollback),
                )
                    .chain()
                    .in_set(PredictionSet::All),
            )
                .chain(),
        );
        app.configure_sets(PreUpdate, PredictionSet::All.run_if(should_run));
        app.add_systems(
            PreUpdate,
            (
                // - we first check via observer if:
                //   - the entity has a matching PreSpawned. If match, remove PrePredicted/ShouldBePredicted.
                //     If no match we do nothing and treat this as a normal-predicted entity
                //   - the entity has a PrePredicted component. If it does, remove ShouldBePredicted to not trigger normal prediction-spawn system
                // - then we check via a system if we should spawn a new predicted entity
                spawn_predicted_entity.in_set(PredictionSet::SpawnPrediction),
                remove_prediction_disable.in_set(PredictionSet::RemoveDisable),
                run_rollback.in_set(PredictionSet::Rollback),
                #[cfg(feature = "metrics")]
                super::rollback::no_rollback
                    .after(PredictionSet::CheckRollback)
                    .in_set(PredictionSet::All)
                    .run_if(not(is_in_rollback)),
            ),
        );
        app.add_observer(despawn_confirmed);

        // FixedPreUpdate
        app.configure_sets(
            FixedPreUpdate,
            PredictionSet::RestoreVisualCorrection.in_set(PredictionSet::All),
        );
        app.configure_sets(FixedPreUpdate, PredictionSet::All.run_if(should_run));

        // FixedUpdate systems
        // 1. Update client tick (don't run in rollback)
        // 2. Run main physics/game fixed-update loop
        // 3. Increment rollback tick (only run in fallback)
        // 4. Update predicted history
        app.configure_sets(
            FixedPostUpdate,
            (
                PredictionSet::EntityDespawn,
                // for prespawned entities that could be spawned during FixedUpdate, we want to add the history
                // right away to avoid rollbacks
                PredictionSet::Sync,
                PredictionSet::UpdateHistory,
                // no need to update the visual state during rollbacks
                PredictionSet::VisualCorrection.run_if(not(is_in_rollback)),
            )
                .in_set(PredictionSet::All)
                .chain(),
        );
        app.configure_sets(FixedPostUpdate, PredictionSet::All.run_if(should_run));

        // NOTE: this needs to run in FixedPostUpdate because the order we want is (if we replicate Position):
        // - Physics update
        // - UpdateHistory
        // - Correction: update Position
        // - Sync: update Transform
        // - VisualInterpolation::UpdateVisualInterpolationState

        // FixedPostUpdate systems
        // 1. Interpolate between the confirmed state and the incorrect predicted state
        // app.configure_sets(
        //     FixedPostUpdate,
        //     PredictionSet::VisualCorrection
        //         // we want visual interpolation to use the corrected state
        //         .before(InterpolationSet::UpdateVisualInterpolationState)
        //         // no need to update the visual state during rollbacks
        //         .run_if(not(is_in_rollback))
        //         .in_set(PredictionSet::All),
        // );

        // PLUGINS
        if !app.is_plugin_added::<crate::shared::SharedPlugin>() {
            app.add_plugins(crate::shared::SharedPlugin);
        }
        app.add_plugins((
            PredictionDiagnosticsPlugin::default(),
            PrePredictionPlugin,
            PreSpawnedPlugin,
            RollbackPlugin,
        ));
    }

    // We run this after `build` and `finish` to make sure that all components were registered before we create the observer
    // that will trigger on all predicted components
    fn cleanup(&self, app: &mut App) {
        add_sync_systems(app);
    }
}
