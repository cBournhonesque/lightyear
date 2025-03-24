use super::pre_prediction::PrePredictionPlugin;
use super::predicted_history::apply_confirmed_update;
use super::resource_history::{
    handle_tick_event_resource_history, update_resource_history, ResourceHistory,
};
use super::rollback::{
    increment_rollback_tick, prepare_rollback, prepare_rollback_non_networked,
    prepare_rollback_prespawn, prepare_rollback_resource, remove_prediction_disable, run_rollback,
    Rollback, RollbackPlugin, RollbackState,
};
use super::spawn::spawn_predicted_entity;
use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::prediction::correction::{
    get_corrected_state, restore_corrected_state, set_original_prediction_post_rollback,
};
use crate::client::prediction::despawn::{despawn_confirmed, PredictionDisable};
use crate::client::prediction::predicted_history::{
    add_prediction_history, add_sync_systems, apply_component_removal_confirmed,
    apply_component_removal_predicted, handle_tick_event_prediction_history,
    update_prediction_history,
};
use crate::client::prediction::prespawn::PreSpawnedPlayerObjectPlugin;
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::Predicted;
use crate::prelude::client::is_connected;
use crate::prelude::{is_host_server, PreSpawned};
use crate::shared::sets::{ClientMarker, InternalMainSet};
use bevy::ecs::component::Mutable;
use bevy::ecs::entity_disabling::DefaultQueryFilters;
use bevy::prelude::*;
use bevy::reflect::Reflect;
use core::time::Duration;

/// Configuration to specify how the prediction plugin should behave
#[derive(Debug, Clone, Copy, Reflect)]
pub struct PredictionConfig {
    /// If true, we always rollback whenever we receive a server update, instead of checking
    /// ff the confirmed state matches the predicted state history
    pub always_rollback: bool,
    /// Minimum number of input delay ticks that will be applied, regardless of latency.
    ///
    /// This should almost always be set to 0 to ensure that your game is as responsive as possible.
    /// Some games might prefer enforcing a minimum input delay to ensure a consistent game feel even
    /// when the latency conditions are changing.
    pub minimum_input_delay_ticks: u16,
    /// Maximum amount of input delay that will be applied in order to cover latency, before any prediction
    /// is done to cover additional latency.
    ///
    /// Input delay can be ideal in low-latency situations to avoid rollbacks and networking artifacts, but it
    /// must be balanced against the responsiveness of the game. Even at higher latencies, it's useful to add
    /// some input delay to reduce the amount of rollback ticks that are needed. (to reduce the rollback visual artifacts
    /// and CPU costs)
    ///
    /// The default value is 3 (or about 50ms at 60Hz): for clients that have less than 50ms ping, we will apply input delay
    /// to cover the latency, and there should no rollback.
    ///
    /// Set to 0ms if you won't want any input delay. (for example for shooters)
    pub maximum_input_delay_before_prediction: u16,
    /// This setting describes how far ahead the client simulation is allowed to predict to cover latency.
    /// This controls the maximum amount of rollback ticks. Any additional latency will be covered by adding more input delays.
    ///
    /// The default value is 7 ticks (or about 100ms of prediction at 60Hz)
    ///
    /// If you set `maximum_input_delay_before_prediction` to 50ms and `maximum_predicted_time` to 100ms, and the client has:
    /// - 30ms ping: there will be 30ms of input delay and no prediction
    /// - 120ms ping: there will be 50ms of input delay and 70ms of prediction/rollback
    /// - 200ms ping: there will be 100ms of input delay, and 100ms of prediction/rollback
    pub maximum_predicted_ticks: u16,
    /// The number of correction ticks will be a multiplier of the number of ticks between
    /// the client and the server correction
    /// (i.e. if the client is 10 ticks head and correction_ticks is 1.0, then the correction will be done over 10 ticks)
    // Number of ticks it will take to visually update the Predicted state to the new Corrected state
    pub correction_ticks_factor: f32,
}

impl Default for PredictionConfig {
    /// By default we don't apply any input delay, because input_delay is only compatible with
    /// the leafwing inputs
    /// (Adding input delay would mess up the client timeline)
    fn default() -> Self {
        Self::no_input_delay()
    }
}

impl PredictionConfig {
    /// Cover up to 50ms of latency with input delay, and after that use prediction for up to 100ms
    /// - `minimum_input_delay_ticks`: no minimum input delay
    /// - `minimum_input_delay_before_prediction`: 3 ticks (or about 50ms at 60Hz), cover 50ms of latency with input delay
    /// - `maximum_predicted_ticks`: 7 ticks (or about 100ms at 60Hz), cover the next 100ms of latency with prediction
    ///   (the rest will be covered by more input delay)
    pub fn balanced() -> Self {
        Self {
            always_rollback: false,
            minimum_input_delay_ticks: 0,
            maximum_input_delay_before_prediction: 3,
            maximum_predicted_ticks: 7,
            correction_ticks_factor: 1.0,
        }
    }

    /// No input-delay, all the latency will be covered by prediction
    pub fn no_input_delay() -> Self {
        Self {
            always_rollback: false,
            minimum_input_delay_ticks: 0,
            maximum_input_delay_before_prediction: 0,
            maximum_predicted_ticks: 100,
            correction_ticks_factor: 1.0,
        }
    }

    /// All the latency will be covered by adding input-delay
    pub fn no_prediction() -> Self {
        Self {
            always_rollback: false,
            minimum_input_delay_ticks: 0,
            maximum_input_delay_before_prediction: 0,
            maximum_predicted_ticks: 0,
            correction_ticks_factor: 0.0,
        }
    }

    pub fn always_rollback(mut self, always_rollback: bool) -> Self {
        self.always_rollback = always_rollback;
        self
    }

    /// Ensures that there is a fixed amount of input delay in all cases
    pub fn set_fixed_input_delay_ticks(&mut self, tick: u16) {
        self.minimum_input_delay_ticks = tick;
        self.maximum_input_delay_before_prediction = tick;
        self.maximum_predicted_ticks = 100;
    }

    /// Update the amount of input delay (number of ticks)
    pub fn with_minimum_input_delay_ticks(mut self, tick: u16) -> Self {
        self.minimum_input_delay_ticks = tick;
        self
    }

    /// Update the amount of input delay (number of ticks)
    pub fn with_correction_ticks_factor(mut self, factor: f32) -> Self {
        self.correction_ticks_factor = factor;
        self
    }

    /// Compute the amount of input delay that should be applied, considering the current RTT
    pub fn input_delay_ticks(&self, rtt: Duration, tick_interval: Duration) -> u16 {
        assert!(self.minimum_input_delay_ticks <= self.maximum_input_delay_before_prediction,
                "The minimum amount of input_delay should be lower than the maximum_input_delay_before_prediction");
        let rtt_ticks = (rtt.as_nanos() as f32 / tick_interval.as_nanos() as f32).ceil() as u16;
        // if the rtt is lower than the minimum input delay, we will apply the minimum input delay
        if rtt_ticks <= self.minimum_input_delay_ticks {
            return self.minimum_input_delay_ticks;
        }
        // else, apply input delay up to the maximum input delay
        if rtt_ticks <= self.maximum_input_delay_before_prediction {
            return rtt_ticks;
        }
        // else, apply input delay up to the maximum input delay, and cover the rest with prediction
        // if not possible, add even more input delay
        if rtt_ticks <= (self.maximum_predicted_ticks + self.maximum_input_delay_before_prediction)
        {
            self.maximum_input_delay_before_prediction
        } else {
            rtt_ticks - self.maximum_predicted_ticks
        }
    }
}

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

    // FixedLast Sets
    /// Increment the rollback tick after the main fixed-update physics loop has run
    IncrementRollbackTick,

    /// General set encompassing all other system sets
    All,
}

/// Returns true if we are doing rollback
pub fn is_in_rollback(rollback: Option<Res<Rollback>>) -> bool {
    rollback.is_some_and(|rollback| rollback.is_rollback())
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

pub fn add_prediction_systems<C: SyncComponent>(app: &mut App, prediction_mode: ComponentSyncMode) {
    match prediction_mode {
        ComponentSyncMode::Full => {
            #[cfg(feature = "metrics")]
            {
                metrics::describe_counter!(format!(
                    "prediction::rollbacks::causes::{}::missing_on_confirmed",
                    core::any::type_name::<C>()
                ), metrics::Unit::Count, "Component present in the prediction history but missing on the confirmed entity");
                metrics::describe_counter!(format!(
                    "prediction::rollbacks::causes::{}::value_mismatch",
                    core::any::type_name::<C>()
                ), metrics::Unit::Count, "Component present in the prediction history but with a different value than on the confirmed entity");
                metrics::describe_counter!(format!(
                    "prediction::rollbacks::causes::{}::missing_on_predicted",
                    core::any::type_name::<C>()
                ), metrics::Unit::Count, "Component present in the confirmed entity but missing in the prediction history");
                metrics::describe_counter!(format!(
                    "prediction::rollbacks::causes::{}::removed_on_predicted",
                    core::any::type_name::<C>()
                ), metrics::Unit::Count, "Component present in the confirmed entity but removed in the prediction history");
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
        ComponentSyncMode::Simple => {
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
        // we only run prediction:
        // - if we're not in host-server mode
        // - after the client is connected
        // NOTE: we need to run the prediction systems even if we're not synced, because we want
        //  our HistoryBuffer to contain values for components/resources that were updated before syncing
        //  is done.
        let should_prediction_run = not(is_host_server).and(is_connected);

        // REFLECTION
        app.register_type::<Predicted>()
            .register_type::<Confirmed>()
            .register_type::<PreSpawned>()
            .register_type::<Rollback>()
            .register_type::<RollbackState>()
            .register_type::<PredictionDisable>()
            .register_type::<PredictionConfig>();

        // RESOURCES
        app.init_resource::<PredictionManager>();
        app.insert_resource(Rollback::new(RollbackState::Default));

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
                InternalMainSet::<ClientMarker>::ReceiveEvents,
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
        )
        .configure_sets(
            PreUpdate,
            PredictionSet::All.run_if(should_prediction_run.clone()),
        );
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
        )
        .configure_sets(
            FixedPreUpdate,
            PredictionSet::All.run_if(should_prediction_run.clone()),
        );

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
        )
        .configure_sets(
            FixedPostUpdate,
            PredictionSet::All.run_if(should_prediction_run.clone()),
        );

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
        app.add_systems(
            FixedLast,
            increment_rollback_tick.in_set(PredictionSet::IncrementRollbackTick),
        );
        app.configure_sets(
            FixedLast,
            PredictionSet::IncrementRollbackTick
                .run_if(is_in_rollback)
                .in_set(PredictionSet::All),
        )
        .configure_sets(
            FixedLast,
            PredictionSet::All.run_if(should_prediction_run.clone()),
        );

        // PLUGINS
        app.add_plugins((
            PrePredictionPlugin,
            PreSpawnedPlayerObjectPlugin,
            RollbackPlugin,
        ));
    }

    // We run this after `build` and `finish` to make sure that all components were registered before we create the observer
    // that will trigger on all predicted components
    fn cleanup(&self, app: &mut App) {
        add_sync_systems(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_delay_config() {
        let config_1 = PredictionConfig {
            always_rollback: false,
            minimum_input_delay_ticks: 2,
            maximum_input_delay_before_prediction: 3,
            maximum_predicted_ticks: 7,
            correction_ticks_factor: 0.0,
        };
        // 1. Test the minimum input delay
        assert_eq!(
            config_1.input_delay_ticks(Duration::from_millis(10), Duration::from_millis(16)),
            2
        );

        // 2. Test the maximum input delay before prediction
        assert_eq!(
            config_1.input_delay_ticks(Duration::from_millis(60), Duration::from_millis(16)),
            3
        );

        // 3. Test the maximum predicted delay
        assert_eq!(
            config_1.input_delay_ticks(Duration::from_millis(200), Duration::from_millis(16)),
            6
        );
        assert_eq!(
            config_1.input_delay_ticks(Duration::from_millis(300), Duration::from_millis(16)),
            12
        );
    }
}
