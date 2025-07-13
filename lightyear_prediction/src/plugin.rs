use super::pre_prediction::PrePredictionPlugin;
use super::predicted_history::apply_confirmed_update;
use super::resource_history::{
    ResourceHistory, handle_tick_event_resource_history, update_resource_history,
};
use super::rollback::{RollbackPlugin, RollbackSet, prepare_rollback, prepare_rollback_resource};
use super::spawn::spawn_predicted_entity;
use crate::despawn::{PredictionDisable, despawn_confirmed};
use crate::diagnostics::PredictionDiagnosticsPlugin;
use crate::manager::PredictionManager;
use crate::predicted_history::{
    add_prediction_history, add_sync_systems, apply_component_removal_confirmed,
    apply_component_removal_predicted, handle_tick_event_prediction_history,
    update_prediction_history,
};
use crate::prespawn::{PreSpawned, PreSpawnedPlugin};
use crate::registry::PredictionRegistry;
use crate::{
    Predicted, PredictionMode, SyncComponent, predicted_on_add_hook, predicted_on_remove_hook,
};
#[cfg(feature = "metrics")]
use alloc::format;
use bevy_app::{App, FixedPostUpdate, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::{
    entity_disabling::DefaultQueryFilters,
    query::{With, Without},
    resource::Resource,
    schedule::{IntoScheduleConfigs, SystemSet},
    system::Query,
};
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::host::HostClient;
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
    /// System set encompassing the sets in [`RollbackSet`]
    Rollback,

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
pub fn add_non_networked_rollback_systems<C: SyncComponent>(app: &mut App) {
    app.add_observer(apply_component_removal_predicted::<C>);
    app.add_observer(add_prediction_history::<C>);
    app.add_systems(
        PreUpdate,
        prepare_rollback::<C>.in_set(RollbackSet::Prepare),
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
        prepare_rollback_resource::<R>.in_set(RollbackSet::Prepare),
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
                (
                    // for SyncMode::Full, we need to check if we need to rollback.
                    // TODO: for mode=simple/once, we still need to re-add the component if the entity ends up not being despawned!
                    // check_rollback::<C>.in_set(PredictionSet::CheckRollback),
                    prepare_rollback::<C>.in_set(RollbackSet::Prepare),
                ),
            );
            app.add_systems(
                FixedPostUpdate,
                (
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
                    apply_confirmed_update::<C>.in_set(PredictionSet::Sync),
                ),
            );
        }
        _ => {}
    };
}

impl Plugin for PredictionPlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.init_resource::<PredictionRegistry>();

        // REFLECTION
        app.register_type::<Predicted>()
            .register_type::<PreSpawned>()
            .register_type::<PredictionDisable>();

        // HOOKS
        app.world_mut()
            .register_component_hooks::<Predicted>()
            .on_add(predicted_on_add_hook)
            .on_remove(predicted_on_remove_hook);

        // OBSERVERS
        app.add_observer(PredictionManager::handle_tick_sync);

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
                    PredictionSet::Rollback,
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
            ),
        );
        app.add_observer(despawn_confirmed);

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
            )
                .in_set(PredictionSet::All)
                .chain(),
        );
        app.configure_sets(FixedPostUpdate, PredictionSet::All.run_if(should_run));

        // PostUpdate
        app.configure_sets(PostUpdate, PredictionSet::All.run_if(should_run));

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
