use crate::client_server::prediction::{
    register_rollback_check_helper, trigger_rollback_check,
    trigger_rollback_check_without_completed_tick, trigger_state_rollback,
};
use crate::protocol::{CompFull, CompNotNetworked, NativeInput};
use crate::stepper::*;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::prelude::*;
use bevy_replicon::prelude::RepliconTick;
use core::time::Duration;
use lightyear::input::native::prelude::InputMarker;
use lightyear::prediction::Predicted;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prelude::input::native::ActionState;
use lightyear_connection::prelude::NetworkTarget;
use lightyear_core::id::PeerId;
use lightyear_core::prelude::{ConfirmedHistory, ConfirmedState, LocalTimeline, Tick};
use lightyear_messages::MessageManager;
use lightyear_prediction::despawn::{PredictionDespawnCommandsExt, PredictionDisable};
use lightyear_prediction::manager::{LastConfirmedInput, RollbackMode, StateRollbackMetadata};
use lightyear_prediction::prelude::*;
use lightyear_prediction::rollback::{DeterministicPredicted, reset_input_rollback_tracker};
use lightyear_replication::prelude::*;
use test_log::test;

fn setup() -> (ClientServerStepper, Entity) {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    register_rollback_check_helper(stepper.client_app());

    // add predicted/confirmed entities
    let predicted = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, CompFull(1.0)))
        .id();
    // run one frame to initialize prediction history for the entity
    stepper.frame_step(1);
    (stepper, predicted)
}

fn insert_confirmed<C: Component + PartialEq>(
    world: &mut World,
    entity: Entity,
    tick: Tick,
    value: Option<C>,
) {
    let state = match value {
        Some(value) => ConfirmedState::Confirmed(value),
        None => ConfirmedState::Removed,
    };
    let mut entity_mut = world.entity_mut(entity);
    if let Some(mut history) = entity_mut.get_mut::<ConfirmedHistory<C>>() {
        history.insert(tick, state);
    } else {
        let mut history = ConfirmedHistory::<C>::default();
        history.insert(tick, state);
        entity_mut.insert(history);
    }
}

fn record_completed_mutate_tick(world: &mut World, replicon_tick: RepliconTick, tick: Tick) {
    let mut checkpoints = world.resource_mut::<ReplicationCheckpointMap>();
    checkpoints.record(replicon_tick, tick);
    checkpoints.record_last_confirmed_tick(replicon_tick);
}

#[derive(Resource, Default)]
struct ObservedRollbackStart(Option<Tick>);

fn record_rollback_start(
    manager: Single<&PredictionManager>,
    mut observed: ResMut<ObservedRollbackStart>,
) {
    if observed.0.is_none() {
        observed.0 = manager.get_rollback_start_tick();
    }
}

fn observe_rollback_start(app: &mut App) {
    app.insert_resource(ObservedRollbackStart::default());
    app.add_systems(
        PreUpdate,
        record_rollback_start
            .after(RollbackSystems::Check)
            .before(RollbackSystems::Prepare),
    );
}

// =============================================================================
// Scenario 1: Non-networked component without confirmed state is not removed on state rollback
// =============================================================================

/// Client prediction adds a non-networked component.
/// On state rollback, missing confirmed state should not be treated as
/// authoritative removal.
#[test]
fn test_non_networked_insert_kept_on_state_rollback_without_confirmed_history() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(1);
    let rollback_tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // Client prediction adds CompNotNetworked (not replicated, server doesn't have it)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompNotNetworked(1.0));

    // Simulate confirmed state at rollback_tick: CompFull still 1.0 (no change from server)
    insert_confirmed(
        stepper.client_app().world_mut(),
        predicted,
        rollback_tick,
        Some(CompFull(1.0)),
    );

    trigger_rollback_check(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    // CompNotNetworked should remain: there was no authoritative removal.
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompNotNetworked>(predicted)
            .is_some(),
        "State rollback should not remove a component only because it has no confirmed history"
    );
}

// =============================================================================
// Scenario 2: Component predicted-removed → re-inserted on rollback
// =============================================================================

/// Client prediction removes a component, but server still has it.
/// On rollback, the component should be restored.
#[test]
fn test_predicted_remove_restored_on_rollback() {
    let (mut stepper, predicted) = setup();

    fn increment_and_remove(
        mut commands: Commands,
        mut query: Query<(Entity, &mut CompFull), With<Predicted>>,
    ) {
        for (entity, mut comp) in query.iter_mut() {
            comp.0 += 1.0;
            if comp.0 == 5.0 {
                commands.entity(entity).remove::<CompFull>();
            }
        }
    }
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_and_remove);

    // Run until CompFull is removed (1.0 → 2.0 → 3.0 → 4.0 → 5.0 → removed)
    stepper.frame_step(5);
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .is_none(),
        "CompFull should have been removed by prediction"
    );

    let tick = stepper.client_tick(0);
    // Simulate server confirms CompFull = -10.0 at tick-3 (server says it still exists)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompFull(-10.0));
    insert_confirmed(
        stepper.client_app().world_mut(),
        predicted,
        tick - 3,
        Some(CompFull(-10.0)),
    );

    trigger_rollback_check(&mut stepper, tick - 3);
    stepper.frame_step(1);

    // CompFull should be re-added and re-simulated: -10 + 4 increments = -6
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .unwrap()
            .0,
        -6.0,
        "Predicted-removed component should be restored and re-simulated on rollback"
    );
}

// =============================================================================
// Scenario 3: Entity predicted-despawned → re-enabled on rollback
// =============================================================================

/// Client uses `prediction_despawn` on an entity.
/// On rollback (to before the despawn), the entity should be re-enabled.
#[test]
fn test_predicted_despawn_restored_on_rollback() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(1);
    let rollback_tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // Predicted-despawn the entity (adds PredictionDisable, doesn't actually despawn)
    stepper
        .client_app()
        .world_mut()
        .commands()
        .entity(predicted)
        .prediction_despawn();
    stepper.frame_step(1);

    assert!(
        stepper.client_app().world().get_entity(predicted).is_ok(),
        "Entity should still exist after prediction_despawn"
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionDisable>(predicted)
            .is_some(),
        "Entity should have PredictionDisable marker"
    );

    // Trigger rollback to before the despawn
    trigger_rollback_check(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    // PredictionDisable should be removed, entity re-enabled
    assert!(
        stepper.client_app().world().get_entity(predicted).is_ok(),
        "Entity should still exist after rollback"
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionDisable>(predicted)
            .is_none(),
        "PredictionDisable should be removed after rollback"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .unwrap(),
        &CompFull(1.0),
        "Component should be restored to value at rollback tick"
    );
}

// =============================================================================
// Scenario 4: Entity predicted-spawned → despawned on rollback
// =============================================================================

/// A DeterministicPredicted entity spawned during prediction should be despawned
/// if rollback goes back to before the spawn tick.
#[test]
fn test_predicted_spawn_despawned_on_rollback() {
    let (mut stepper, _) = setup();
    stepper.frame_step(1);

    let tick = stepper.client_tick(0);
    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, DeterministicPredicted::default(), CompFull(1.0)))
        .id();

    // trigger a rollback to before the entity was spawned
    trigger_state_rollback(&mut stepper, tick - 1);
    stepper.frame_step(1);

    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(predicted_a)
            .is_err(),
        "Predicted-spawned entity should be despawned on rollback to before spawn tick"
    );
}

// =============================================================================
// Scenario 5: Component modified → corrected on rollback
// =============================================================================

/// Client prediction modifies a component value differently than the server.
/// On rollback, the component should snap to the confirmed value and re-simulate.
#[test]
fn test_predicted_modify_corrected_on_rollback() {
    let (mut stepper, predicted) = setup();

    fn increment_component(mut query: Query<&mut CompFull, With<Predicted>>) {
        for mut comp in query.iter_mut() {
            comp.0 += 1.0;
        }
    }
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_component);

    // Run 3 frames: CompFull goes 1.0 → 2.0 → 3.0 → 4.0
    stepper.frame_step(3);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .unwrap()
            .0,
        4.0
    );

    let tick = stepper.client_tick(0);
    // Server says CompFull was actually 10.0 at tick-2 (different from predicted 2.0)
    insert_confirmed(
        stepper.client_app().world_mut(),
        predicted,
        tick - 2,
        Some(CompFull(10.0)),
    );

    trigger_rollback_check(&mut stepper, tick - 2);
    stepper.frame_step(1);

    // Snap to 10.0 at tick-2, re-simulate 3 ticks: 10.0 + 3 = 13.0
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .unwrap()
            .0,
        13.0,
        "Modified component should be corrected to confirmed value and re-simulated"
    );
}

/// If one predicted entity triggers a rollback from an older tick while another
/// predicted entity already has a newer confirmed tick, the newer confirmed
/// value must be preserved during replay.
#[test]
fn test_rollback_preserves_later_confirmed_values_on_other_entities() {
    fn increment_component(mut query: Query<&mut CompFull, With<Predicted>>) {
        for mut comp in query.iter_mut() {
            comp.0 += 1.0;
        }
    }

    let (mut stepper, predicted_a) = setup();
    let predicted_b = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, CompFull(10.0)))
        .id();

    // Initialize prediction history for the second entity.
    stepper.frame_step(1);
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_component);

    // Build enough history so rollback and later confirmed ticks are distinct.
    stepper.frame_step(4);
    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 3;
    let later_confirmed_tick = current_tick - 1;

    let rollback_replicon_tick = RepliconTick::new(u32::from(rollback_tick.0));
    let later_replicon_tick = RepliconTick::new(u32::from(later_confirmed_tick.0));

    let world = stepper.client_app().world_mut();
    world
        .resource_mut::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
        .record(RepliconTick::default(), rollback_tick);
    world
        .resource_mut::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
        .record(rollback_replicon_tick, rollback_tick);
    world
        .resource_mut::<lightyear_replication::checkpoint::ReplicationCheckpointMap>()
        .record(later_replicon_tick, later_confirmed_tick);
    insert_confirmed(world, predicted_a, rollback_tick, Some(CompFull(100.0)));
    world
        .entity_mut(predicted_a)
        .insert(ConfirmHistory::new(rollback_replicon_tick));

    insert_confirmed(
        world,
        predicted_b,
        later_confirmed_tick,
        Some(CompFull(200.0)),
    );
    world
        .entity_mut(predicted_b)
        .insert(ConfirmHistory::new(later_replicon_tick));

    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_a)
            .unwrap()
            .0,
        104.0,
        "Rollback initiator should replay from the older confirmed tick through the current frame tick"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_b)
            .unwrap()
            .0,
        203.0,
        "Later confirmed value on another entity should be preserved and replayed across the remaining rollback ticks and the current frame tick"
    );
}

/// Multiple explicit confirmed samples for the same component should survive an
/// older rollback. The rollback starts at the older mismatch, then replay still
/// snaps to the later confirmed sample.
#[test]
fn test_batched_confirmed_values_survive_older_rollback() {
    fn increment_component(mut query: Query<&mut CompFull, With<Predicted>>) {
        for mut comp in query.iter_mut() {
            comp.0 += 1.0;
        }
    }

    let (mut stepper, predicted) = setup();
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_component);

    stepper.frame_step(4);
    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 3;
    let later_confirmed_tick = current_tick - 1;

    let world = stepper.client_app().world_mut();
    insert_confirmed(world, predicted, rollback_tick, Some(CompFull(100.0)));
    insert_confirmed(
        world,
        predicted,
        later_confirmed_tick,
        Some(CompFull(200.0)),
    );

    trigger_rollback_check(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    let confirmed_history = stepper
        .client_app()
        .world()
        .get::<ConfirmedHistory<CompFull>>(predicted)
        .unwrap();
    assert!(
        confirmed_history
            .get_state_at(later_confirmed_tick)
            .is_some(),
        "later confirmed sample should survive rollback preparation"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .unwrap()
            .0,
        203.0,
        "rollback replay should still snap to the later confirmed sample"
    );
}

#[test]
fn test_future_confirmed_value_is_not_checked_by_unchanged_completed_tick() {
    let (mut stepper, predicted) = setup();
    observe_rollback_start(stepper.client_app());

    let future_tick = stepper.client_tick(0) + 2;
    let future_replicon_tick = RepliconTick::new(future_tick.0);
    let world = stepper.client_app().world_mut();
    record_completed_mutate_tick(world, future_replicon_tick, future_tick);
    world
        .entity_mut(predicted)
        .insert(ConfirmHistory::new(future_replicon_tick));
    insert_confirmed(world, predicted, future_tick, Some(CompFull(10.0)));

    stepper.frame_step(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<ObservedRollbackStart>()
            .0,
        None,
        "future confirmed sample should not rollback before local prediction reaches its tick"
    );

    stepper.frame_step(2);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<ObservedRollbackStart>()
            .0,
        None,
        "explicitly confirmed samples are skipped by the unchanged completed-tick scan"
    );
}

#[test]
fn test_future_confirmed_insert_is_not_checked_by_unchanged_completed_tick() {
    let (mut stepper, predicted) = setup();
    observe_rollback_start(stepper.client_app());

    let future_tick = stepper.client_tick(0) + 2;
    let future_replicon_tick = RepliconTick::new(future_tick.0);
    let world = stepper.client_app().world_mut();
    world.entity_mut(predicted).remove::<CompFull>();
    record_completed_mutate_tick(world, future_replicon_tick, future_tick);
    world
        .entity_mut(predicted)
        .insert(ConfirmHistory::new(future_replicon_tick));
    insert_confirmed(world, predicted, future_tick, Some(CompFull(10.0)));

    stepper.frame_step(3);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<ObservedRollbackStart>()
            .0,
        None,
        "explicitly confirmed inserts are skipped by the unchanged completed-tick scan"
    );
}

/// A completed mutate tick is a global confirmation point. If one entity receives an
/// explicit update at that tick and another entity does not, the unchanged entity should
/// still be checked against its last confirmed value at the completed tick.
#[test]
fn test_completed_mutate_tick_checks_unchanged_entities() {
    fn increment_component(mut query: Query<&mut CompFull, With<Predicted>>) {
        for mut comp in query.iter_mut() {
            comp.0 += 1.0;
        }
    }

    let (mut stepper, updated) = setup();
    let unchanged = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, CompFull(10.0)))
        .id();

    stepper.frame_step(1);
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_component);
    observe_rollback_start(stepper.client_app());

    // Build a few ticks of prediction history after initializing the second entity.
    stepper.frame_step(4);
    let completed_tick = stepper.client_tick(0) - 2;
    let previous_confirmed_tick = completed_tick - 1;

    let updated_replicon_tick = RepliconTick::new(700);
    let previous_replicon_tick = RepliconTick::new(699);

    let world = stepper.client_app().world_mut();
    record_completed_mutate_tick(world, updated_replicon_tick, completed_tick);
    world
        .resource_mut::<ReplicationCheckpointMap>()
        .record(previous_replicon_tick, previous_confirmed_tick);

    insert_confirmed(world, updated, completed_tick, Some(CompFull(1.0)));
    world
        .entity_mut(updated)
        .insert(ConfirmHistory::new(updated_replicon_tick));

    insert_confirmed(
        world,
        unchanged,
        previous_confirmed_tick,
        Some(CompFull(20.0)),
    );
    world
        .entity_mut(unchanged)
        .insert(ConfirmHistory::new(previous_replicon_tick));

    stepper.frame_step(1);

    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<ObservedRollbackStart>()
            .0,
        Some(completed_tick),
        "Unchanged entity mismatch should roll back from the completed mutate tick"
    );
}

#[test]
fn test_future_completed_mutate_tick_is_not_marked_processed() {
    let (mut stepper, _) = setup();

    stepper.frame_step(3);
    let future_tick = stepper.client_tick(0) + 1_000;
    let future_replicon_tick = RepliconTick::new(920);
    record_completed_mutate_tick(
        stepper.client_app().world_mut(),
        future_replicon_tick,
        future_tick,
    );

    stepper.frame_step(1);

    assert_ne!(
        stepper
            .client_app()
            .world()
            .resource::<StateRollbackMetadata>()
            .last_processed_tick(),
        Some(future_tick),
        "A future completed mutate tick must not be marked processed before it can be checked"
    );
}

#[test]
fn test_explicit_mismatch_waits_for_completed_mutate_tick() {
    let (mut stepper, _) = setup();
    observe_rollback_start(stepper.client_app());

    stepper.frame_step(5);
    let completed_tick = stepper.client_tick(0) - 4;
    let mismatch_tick = stepper.client_tick(0) - 2;
    let completed_replicon_tick = RepliconTick::new(930);
    record_completed_mutate_tick(
        stepper.client_app().world_mut(),
        completed_replicon_tick,
        completed_tick,
    );

    trigger_rollback_check_without_completed_tick(&mut stepper, mismatch_tick);
    stepper.frame_step(1);

    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<ObservedRollbackStart>()
            .0,
        None,
        "Explicit mismatch should wait until a completed mutate tick reaches the mismatch"
    );

    record_completed_mutate_tick(
        stepper.client_app().world_mut(),
        RepliconTick::new(931),
        mismatch_tick,
    );
    stepper.frame_step(1);

    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<ObservedRollbackStart>()
            .0,
        Some(mismatch_tick),
        "Explicit mismatch should rollback once that exact tick is the completed mutate tick"
    );
}

/// When a completed mutate tick triggers rollback through an unchanged entity, later explicit
/// confirmed values on other entities must be preserved for replay.
#[test]
fn test_completed_mutate_tick_rollback_preserves_later_confirmed_values() {
    fn increment_component(mut query: Query<&mut CompFull, With<Predicted>>) {
        for mut comp in query.iter_mut() {
            comp.0 += 1.0;
        }
    }

    let (mut stepper, predicted_a) = setup();
    let predicted_b = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, CompFull(10.0)))
        .id();
    let predicted_c = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, CompFull(20.0)))
        .id();

    // Initialize prediction history for the second entity.
    stepper.frame_step(1);
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_component);
    observe_rollback_start(stepper.client_app());

    stepper.frame_step(4);
    let current_tick = stepper.client_tick(0);
    let rollback_tick = current_tick - 3;
    let previous_confirmed_tick = rollback_tick - 1;
    let later_confirmed_tick = current_tick - 1;

    let previous_replicon_tick = RepliconTick::new(previous_confirmed_tick.0);
    let later_replicon_tick = RepliconTick::new(later_confirmed_tick.0);
    let same_replicon_tick = RepliconTick::new(rollback_tick.0);

    let world = stepper.client_app().world_mut();
    record_completed_mutate_tick(world, same_replicon_tick, rollback_tick);
    {
        let mut checkpoints = world.resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(previous_replicon_tick, previous_confirmed_tick);
        checkpoints.record(later_replicon_tick, later_confirmed_tick);
    }

    insert_confirmed(
        world,
        predicted_a,
        previous_confirmed_tick,
        Some(CompFull(100.0)),
    );
    world
        .entity_mut(predicted_a)
        .insert(ConfirmHistory::new(previous_replicon_tick));

    insert_confirmed(
        world,
        predicted_b,
        later_confirmed_tick,
        Some(CompFull(200.0)),
    );
    world
        .entity_mut(predicted_b)
        .insert(ConfirmHistory::new(later_replicon_tick));

    insert_confirmed(world, predicted_c, rollback_tick, Some(CompFull(300.0)));
    world
        .entity_mut(predicted_c)
        .insert(ConfirmHistory::new(same_replicon_tick));

    stepper.frame_step(1);

    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<ObservedRollbackStart>()
            .0,
        Some(rollback_tick),
        "Unchanged entity mismatch should roll back from the completed mutate tick"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_a)
            .unwrap()
            .0,
        104.0,
        "Unchanged entity should replay from the completed mutate tick"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_b)
            .unwrap()
            .0,
        203.0,
        "Later confirmed value on another entity should be preserved across rollback"
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_c)
            .unwrap()
            .0,
        304.0,
        "Entity already confirmed at the completed mutate tick should replay from that confirmed value"
    );
}

/// Replicated helper/input entities can carry `ConfirmHistory` without having any
/// prediction history. A stale confirm tick for those entities should not make
/// the unchanged-entity rollback pass resolve an evicted checkpoint.
#[test]
fn test_stale_confirm_history_without_prediction_history_is_ignored() {
    let (mut stepper, _) = setup();
    stepper.frame_step(5);

    let server_confirmed_tick = stepper.client_tick(0) - 1;
    let current_replicon_tick = RepliconTick::new(500);
    let stale_replicon_tick = RepliconTick::new(1);

    let world = stepper.client_app().world_mut();
    world.spawn((Predicted, ConfirmHistory::new(stale_replicon_tick)));
    record_completed_mutate_tick(world, current_replicon_tick, server_confirmed_tick);

    stepper.frame_step(1);
}

#[test]
fn test_missing_confirm_history_checkpoint_mapping_does_not_request_rollback() {
    #[derive(Resource, Default)]
    struct RollbackObserved(bool);

    fn record_rollback(
        manager: Single<&PredictionManager>,
        mut observed: ResMut<RollbackObserved>,
    ) {
        observed.0 |= manager.get_rollback_start_tick().is_some();
    }

    let (mut stepper, predicted) = setup();
    stepper.frame_step(5);

    let server_confirmed_tick = stepper.client_tick(0) - 1;
    let current_replicon_tick = RepliconTick::new(500);
    let stale_replicon_tick = RepliconTick::new(1);

    stepper
        .client_app()
        .insert_resource(RollbackObserved::default());
    stepper.client_app().add_systems(
        PreUpdate,
        record_rollback
            .after(RollbackSystems::Check)
            .before(RollbackSystems::Prepare),
    );

    let world = stepper.client_app().world_mut();
    world
        .entity_mut(predicted)
        .insert(ConfirmHistory::new(stale_replicon_tick));
    record_completed_mutate_tick(world, current_replicon_tick, server_confirmed_tick);

    stepper.frame_step(1);

    assert!(
        !stepper
            .client_app()
            .world()
            .resource::<RollbackObserved>()
            .0
    );
}

// =============================================================================
// Scenario 6: Component inserted from remote → applied on rollback
// =============================================================================

/// Server sends a new component that the client didn't have.
/// On rollback, the component should be present at the confirmed tick.
#[test]
fn test_remote_insert_applied_on_rollback() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(2);
    let tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // Simulate server sending CompNotNetworked(5.0) at `tick` (component client didn't predict)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert(CompNotNetworked(5.0));
    // Create histories for CompNotNetworked with confirmed value
    let mut confirmed_history = ConfirmedHistory::<CompNotNetworked>::default();
    confirmed_history.insert_present(tick, CompNotNetworked(5.0));
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .insert((
            PredictionHistory::<CompNotNetworked>::default(),
            confirmed_history,
        ));

    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);

    // The remotely-inserted component should be present after rollback
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompNotNetworked>(predicted)
            .unwrap(),
        &CompNotNetworked(5.0),
        "Remotely-inserted component should be present after rollback"
    );
}

// =============================================================================
// Scenario 7: Component removed from remote → removed on rollback
// =============================================================================

/// Server removes a component that the client still had.
/// On rollback, the component should be absent.
#[test]
fn test_remote_remove_applied_on_rollback() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(1);
    let rollback_tick = stepper.client_tick(0);
    stepper.frame_step(1);

    // Simulate server removing CompFull at rollback_tick
    insert_confirmed::<CompFull>(
        stepper.client_app().world_mut(),
        predicted,
        rollback_tick,
        None,
    );
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted)
        .remove::<CompFull>();

    trigger_rollback_check(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    // CompFull should be absent after rollback: server confirmed removal
    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .is_none(),
        "Remotely-removed component should be absent after rollback"
    );
}

/// If a predicted component has no history sample at-or-before the rollback
/// tick, rollback should not treat that absence as an authoritative removal.
#[test]
fn test_predicted_component_kept_when_no_history_sample_at_rollback_tick() {
    let (mut stepper, predicted) = setup();

    stepper.frame_step(3);
    let tick = stepper.client_tick(0);

    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .is_some(),
        "precondition: component is present before the rollback"
    );

    {
        let world = stepper.client_app().world_mut();
        let mut history = world
            .get_mut::<PredictionHistory<CompFull>>(predicted)
            .expect("prediction history exists for predicted component");
        history.clear();
        history.add_predicted(tick + 5, Some(CompFull(1.0)));
    }

    let rollback_tick = tick - 2;
    trigger_state_rollback(&mut stepper, rollback_tick);
    stepper.frame_step(1);

    assert!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .is_some(),
        "present predicted component should not be removed when no history sample exists at-or-before the rollback tick"
    );
}

// =============================================================================
// Other rollback tests
// =============================================================================

/// If we have disable_rollback (DeterministicPredicted):
/// 1) the entity alone doesn't trigger rollback
/// 2) if a rollback happens (from another entity), we reset to the predicted history value
#[test]
fn test_disable_rollback() {
    let (mut stepper, predicted_b) = setup();

    // add a DeterministicPredicted entity (disable state rollback for it)
    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, DeterministicPredicted::default(), CompFull(1.0)))
        .id();

    // value gets synced and added to PredictionHistory
    stepper.frame_step(1);

    // 2. If a rollback happens (triggered by predicted_b), DeterministicPredicted entity
    //    gets reset to its historical value
    let tick = stepper.client_tick(0);

    // Set up history for predicted_a with a known confirmed value
    insert_confirmed(
        stepper.client_app().world_mut(),
        predicted_a,
        tick,
        Some(CompFull(10.0)),
    );

    // Simulate confirmed update for predicted_b with a different value to trigger mismatch
    insert_confirmed(
        stepper.client_app().world_mut(),
        predicted_b,
        tick,
        Some(CompFull(3.0)),
    );
    stepper
        .client_app()
        .world_mut()
        .entity_mut(predicted_b)
        .get_mut::<CompFull>()
        .unwrap()
        .0 = 3.0;

    // step once to avoid a 0-tick rollback
    stepper.frame_step(1);

    trigger_rollback_check(&mut stepper, tick);
    stepper.frame_step(1);

    // the DeterministicPredicted entity was rolled back to the past PredictionHistory value
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_a)
            .unwrap()
            .0,
        10.0
    );
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted_b)
            .unwrap()
            .0,
        3.0
    );
}

/// Test that:
/// - the `Time` resource's elapsed is rollbacked to the first tick of the rollback
/// - the `Time` resource's elapsed time is advanced correctly during the rollback
/// - the `Time` resource's delta during a rollback is the `Time<Fixed>`'s delta
#[test]
fn test_rollback_time_resource() {
    #[derive(Debug, PartialEq)]
    struct TimeSnapshot {
        is_rollback: bool,
        delta: Duration,
        elapsed: Duration,
    }

    #[derive(Resource, Default, Debug)]
    struct TimeTracker {
        snapshots: Vec<TimeSnapshot>,
    }

    // Record the time resource's values for each tick.
    fn track_time(
        time: Res<Time>,
        mut time_tracker: ResMut<TimeTracker>,
        rollback: Single<&PredictionManager>,
    ) {
        time_tracker.snapshots.push(TimeSnapshot {
            is_rollback: rollback.is_rollback(),
            delta: time.delta(),
            elapsed: time.elapsed(),
        });
    }

    let (mut stepper, predicted) = setup();
    // Build up enough prediction history so rollback tick is within range
    stepper.frame_step(2);

    // Add time tracking AFTER building history to only capture the rollback frame
    stepper.client_app().insert_resource(TimeTracker::default());
    stepper.client_app().add_systems(FixedUpdate, track_time);
    let time_before_next_tick = *stepper.client_app().world().resource::<Time<Fixed>>();

    // Trigger 2 rollback ticks
    let tick = stepper.client_tick(0);
    trigger_rollback_check(&mut stepper, tick - 2);
    stepper.frame_step(1);

    // Check that the component got synced.
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(predicted)
            .unwrap(),
        &CompFull(1.0)
    );

    // Verify that the 2 rollback ticks and regular tick occurred with the
    // correct delta times and elapsed times.
    let tick_duration = stepper.tick_duration;
    let time_tracker = stepper.client_app().world().resource::<TimeTracker>();
    assert_eq!(
        time_tracker.snapshots,
        vec![
            TimeSnapshot {
                is_rollback: true,
                delta: tick_duration,
                elapsed: time_before_next_tick.elapsed() - tick_duration
            },
            TimeSnapshot {
                is_rollback: true,
                delta: tick_duration,
                elapsed: time_before_next_tick.elapsed()
            },
            TimeSnapshot {
                is_rollback: false,
                delta: tick_duration,
                elapsed: time_before_next_tick.elapsed() + tick_duration
            }
        ]
    );
}

/// Clients 1 and 2 have inputs and send them to the Server, who rebroadcasts to client 0
fn setup_stepper_for_input_rollback(
    mode: RollbackMode,
) -> (ClientServerStepper, Entity, Entity, Entity, Entity) {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(3));

    let mut client_mut = stepper.client_mut(0);
    let mut prediction_manager = client_mut.get_mut::<PredictionManager>().unwrap();
    prediction_manager.rollback_policy.input = mode;
    prediction_manager.rollback_policy.state = RollbackMode::Disabled;

    let server_entity_1 = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::AllExceptSingle(
            PeerId::Netcode(2),
        )))
        .id();
    let server_entity_2 = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::AllExceptSingle(
            PeerId::Netcode(1),
        )))
        .id();
    stepper.frame_step_server_first(1);

    // Check that in PostUpdate, the LastConfirmedInput is reset if no input messages were received
    let client = stepper.client(0);
    assert!(!client.get::<LastConfirmedInput>().unwrap().received_input());

    // add input-markers on client 1/2 so that they can send remote input messages
    let client_entity_1 = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_1)
        .expect("entity was not replicated to client");
    stepper.client_apps[1]
        .world_mut()
        .entity_mut(client_entity_1)
        .insert((InputMarker::<NativeInput>::default(),));

    let client_entity_2 = stepper
        .client(2)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_2)
        .expect("entity was not replicated to client");
    stepper.client_apps[2]
        .world_mut()
        .entity_mut(client_entity_2)
        .insert((InputMarker::<NativeInput>::default(),));

    let client_entity_a = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_1)
        .expect("entity was not replicated to client");
    // we want to predict this entity
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client_entity_a)
        .insert((CompNotNetworked(1.0), DeterministicPredicted::default()));
    let client_entity_b = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity_2)
        .expect("entity was not replicated to client");

    // build a steady state where we have already received an input
    stepper.frame_step(2);

    (
        stepper,
        client_entity_1,
        client_entity_2,
        client_entity_a,
        client_entity_b,
    )
}

/// Test that we rollback from the last confirmed input when RollbackMode::Always for inputs
#[test]
fn test_input_rollback_always_mode() {
    let (mut stepper, _, _, client_entity, _) =
        setup_stepper_for_input_rollback(RollbackMode::Always);

    // build a steady state where have already received an input
    stepper.frame_step(2);

    // send input message from client 1/2 to server
    stepper.frame_step(1);
    let input_tick = stepper.client_tick(1);

    info!("Will check rollback at tick: {input_tick:?}");

    let check_rollback_start =
        move |timeline: Res<LocalTimeline>,
              manager: Single<(&LastConfirmedInput, &PredictionManager)>| {
            let (last_confirmed_input, manager) = manager.into_inner();
            let tick = timeline.tick();
            if tick == input_tick {
                assert!(last_confirmed_input.received_input());
                let rollback_start = manager.get_rollback_start_tick();
                // We receive the input message for tick `input_tick`, but we rollback at the previous LastConfirmedInput tick,
                // which is `input_tick - 1`
                assert_eq!(rollback_start.unwrap(), input_tick - 1);
            }
        };
    stepper.client_apps[0].add_systems(
        PreUpdate,
        check_rollback_start
            .after(RollbackSystems::Check)
            .before(reset_input_rollback_tracker),
    );

    // modify the CompNotNetworked component
    stepper.client_apps[0]
        .world_mut()
        .get_mut::<CompNotNetworked>(client_entity)
        .unwrap()
        .0 = 2.0;

    // server broadcast input message to clients (including client 0)
    stepper.frame_step_server_first(1);

    // after the rollback, the last_confirmed_input is reset
    assert_eq!(
        stepper
            .client(0)
            .get::<LastConfirmedInput>()
            .unwrap()
            .tick
            .get(),
        input_tick
    );
    // also check that the component was reset to the value it had in the history
    assert_eq!(
        stepper.client_apps[0]
            .world()
            .get::<CompNotNetworked>(client_entity)
            .unwrap()
            .0,
        1.0
    );
}

/// Test that LastConfirmedInput computes the earliest input across multiple clients
#[test]
fn test_last_confirmed_input_multiple_clients() {
    let (mut stepper, client_entity_1, _, _, _) =
        setup_stepper_for_input_rollback(RollbackMode::Always);

    // only client 2 will send an input message to the server
    stepper.client_apps[1]
        .world_mut()
        .entity_mut(client_entity_1)
        .remove::<InputMarker<NativeInput>>();
    stepper.frame_step(1);
    let input_tick = stepper.client_tick(1);

    // server broadcast input message to clients
    stepper.frame_step_server_first(1);

    // after the rollback, the last_confirmed_input is updated. It's updated to `input_tick - 1` and not `input_tick`
    // because we didn't receive a new input message from client 1
    assert_eq!(
        stepper
            .client(0)
            .get::<LastConfirmedInput>()
            .unwrap()
            .tick
            .get(),
        input_tick - 1
    );
}

/// Test that rollback tick is set to the earliest mismatch when RollbackMode::Check for inputs
#[test]
fn test_input_rollback_check_mode_earliest_mismatch() {
    let (mut stepper, client_entity_1, _, client_entity_a, _) =
        setup_stepper_for_input_rollback(RollbackMode::Check);

    // build a steady state where we have already received an input
    stepper.frame_step(2);

    // client 1 and client 2 send an input message to the server
    // client 1's input will cause a mismatch
    stepper.client_apps[1]
        .world_mut()
        .get_mut::<ActionState<NativeInput>>(client_entity_1)
        .unwrap()
        .0 = NativeInput(1);
    stepper.frame_step(1);
    let input_tick = stepper.client_tick(1);

    let check_rollback_start =
        move |timeline: Res<LocalTimeline>, manager: Single<&PredictionManager>| {
            let manager = manager.into_inner();
            let tick = timeline.tick();
            if tick == input_tick {
                assert!(manager.earliest_mismatch_input.has_mismatches());
                let rollback_start = manager.get_rollback_start_tick();
                // there is a mismatch only for client 1, which is enough to trigger a rollback.
                // we trigger a rollback to the earliest mismatch, which is `input_tick`
                assert_eq!(rollback_start.unwrap(), input_tick - 1);
            }
        };
    stepper.client_apps[0].add_systems(
        PreUpdate,
        check_rollback_start
            .after(RollbackSystems::Check)
            .before(reset_input_rollback_tracker),
    );

    // server broadcast input message to clients
    stepper.frame_step_server_first(1);
}

/// Test that we don't rollback if there are no input mismatches in Check mode
#[test]
fn test_no_rollback_without_input_mismatches() {
    let (mut stepper, _, _, _, _) = setup_stepper_for_input_rollback(RollbackMode::Check);

    // build a steady state where we have already received an input
    stepper.frame_step(2);

    // client 1 and client 2 send an input message to the server
    // there will be no mismatches
    stepper.frame_step(1);
    let input_tick = stepper.client_tick(1);

    let check_rollback_start =
        move |timeline: Res<LocalTimeline>, manager: Single<&PredictionManager>| {
            let manager = manager.into_inner();
            let tick = timeline.tick();
            if tick == input_tick {
                assert!(!manager.earliest_mismatch_input.has_mismatches());
                let rollback_start = manager.get_rollback_start_tick();
                assert!(rollback_start.is_none());
            }
        };
    stepper.client_apps[0].add_systems(
        PreUpdate,
        check_rollback_start
            .after(RollbackSystems::Check)
            .before(RollbackSystems::Prepare),
    );

    // server broadcast input message to clients
    stepper.frame_step_server_first(1);
}

/// Test that if we spawn a DeterministicPredicted entity with skip_despawn = true
/// We only start enabling rollback for this entity a few ticks after it was spawned.
#[test]
fn test_deterministic_predicted_skip_despawn() {
    let (mut stepper, _) = setup();

    // add predicted/confirmed entities
    let receiver = stepper.client(0).id();
    let tick = stepper.client_tick(0);
    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((
            Predicted,
            DeterministicPredicted {
                skip_despawn: true,
                enable_rollback_after: 2,
            },
            CompFull(1.0),
        ))
        .id();

    // Rollback: the entity should have DisableRollback added until the
    // configured enable_rollback_after tick.
    trigger_state_rollback(&mut stepper, tick);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .get::<DisableRollback>(predicted_a)
            .is_some()
    );

    // trigger a rollback at tick + 2, we should re-enable rollback
    // since it's the spawn_tick of DeterministicPredicted + 2
    trigger_state_rollback(&mut stepper, tick + 2);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .get::<DisableRollback>(predicted_a)
            .is_none()
    );
}

/// Test that if we spawn a DeterministicPredicted entity with skip_despawn = false
/// The entity is despawned it was spawned before the rollback tick.
#[test]
fn test_deterministic_predicted_despawn() {
    let (mut stepper, _) = setup();
    stepper.frame_step(1);

    // add predicted/confirmed entities
    let receiver = stepper.client(0).id();
    let tick = stepper.client_tick(0);

    let predicted_a = stepper
        .client_app()
        .world_mut()
        .spawn((Predicted, DeterministicPredicted::default(), CompFull(1.0)))
        .id();

    // trigger a rollback at tick - 2, we should despawn the DeterministicPredicted
    // since it was spawned before the rollback
    trigger_state_rollback(&mut stepper, tick - 1);
    stepper.frame_step(1);
    assert!(
        stepper
            .client_app()
            .world()
            .get_entity(predicted_a)
            .is_err()
    )
}
