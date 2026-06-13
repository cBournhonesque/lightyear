use crate::protocol::CompRepliconDiff;
use crate::stepper::*;
use bevy::prelude::*;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{RepliconPlugins, RepliconTick, RuleFns};
use bevy_replicon::shared::replication::diff::{DiffEntityExt, DiffWire};
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;
use lightyear::prelude::{
    InterpolationPlugin, InterpolationRegistrationExt, PredictionRegistrationExt,
};
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::prelude::{ConfirmedHistory, ConfirmedState, Interpolated};
use lightyear_messages::MessageManager;
use lightyear_prediction::Predicted;
use lightyear_prediction::manager::PredictionManager;
use lightyear_prediction::plugin::PredictionPlugin;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::prelude::{
    AppComponentExt, InterpolationTarget, PredictionTarget, Replicate,
};

fn client_entity(stepper: &ClientServerStepper, server_entity: Entity) -> Entity {
    stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap()
}

fn newest_confirmed_value(app: &App, entity: Entity) -> Option<u32> {
    app.world()
        .entity(entity)
        .get::<ConfirmedHistory<CompRepliconDiff>>()
        .and_then(ConfirmedHistory::newest_present)
        .map(|(_, value)| value.0)
}

/// Verifies the end-to-end prediction path for Replicon diff components:
/// server-side `apply_patch` produces patch replication, and the client stores
/// the materialized value in `ConfirmedHistory`.
#[test]
fn diff_prediction_records_confirmed_history_from_replicon_patches() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            CompRepliconDiff(0),
        ))
        .id();
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .apply_patch::<CompRepliconDiff>(0)
        .unwrap();

    stepper.frame_step_server_first(1);
    let client_entity = client_entity(&stepper, server_entity);
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .contains::<Predicted>()
    );
    assert_eq!(
        newest_confirmed_value(stepper.client_app(), client_entity),
        Some(0)
    );

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .apply_patch::<CompRepliconDiff>(1)
        .unwrap();
    stepper.frame_step_server_first(1);

    assert_eq!(
        newest_confirmed_value(stepper.client_app(), client_entity),
        Some(1)
    );
}

/// Verifies the end-to-end interpolation path for Replicon diff components:
/// server-side `apply_patch` produces patch replication, and the client stores
/// the materialized value in `ConfirmedHistory`.
#[test]
fn diff_interpolation_records_confirmed_history_from_replicon_patches() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
            CompRepliconDiff(0),
        ))
        .id();
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .apply_patch::<CompRepliconDiff>(0)
        .unwrap();

    stepper.frame_step_server_first(1);
    let client_entity = client_entity(&stepper, server_entity);
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .contains::<Interpolated>()
    );
    assert_eq!(
        newest_confirmed_value(stepper.client_app(), client_entity),
        Some(0)
    );

    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .apply_patch::<CompRepliconDiff>(1)
        .unwrap();
    stepper.frame_step_server_first(1);

    assert_eq!(
        newest_confirmed_value(stepper.client_app(), client_entity),
        Some(1)
    );
}

fn diff_wire(wire: DiffWire<CompRepliconDiff, u32>) -> Bytes {
    let mut message = Vec::new();
    postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
    message.into()
}

fn setup_prediction_receive_app() -> (App, bevy_replicon::shared::replication::registry::FnsId) {
    let mut app = App::new();
    app.add_plugins((
        bevy::state::app::StatesPlugin,
        RepliconPlugins,
        PredictionPlugin,
    ));
    app.insert_resource(lightyear_core::prelude::LocalTimeline::default());
    app.insert_resource(ReplicationCheckpointMap::default());
    app.world_mut().spawn(PredictionManager::default());
    app.world_mut().flush();
    app.register_component_diff::<CompRepliconDiff>()
        .add_prediction_diff();

    let fns_id = app
        .world_mut()
        .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
            let (_, fns_id) =
                registry.register_rule_fns(world, RuleFns::<CompRepliconDiff>::new_diff());
            fns_id
        });
    (app, fns_id)
}

fn setup_interpolation_receive_app() -> (App, bevy_replicon::shared::replication::registry::FnsId) {
    let mut app = App::new();
    app.add_plugins((
        bevy::state::app::StatesPlugin,
        RepliconPlugins,
        InterpolationPlugin,
    ));
    app.insert_resource(ReplicationCheckpointMap::default());
    app.register_component_diff::<CompRepliconDiff>()
        .add_custom_interpolation_diff();

    let fns_id = app
        .world_mut()
        .resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
            let (_, fns_id) =
                registry.register_rule_fns(world, RuleFns::<CompRepliconDiff>::new_diff());
            fns_id
        });
    (app, fns_id)
}

fn record_checkpoint(app: &mut App, tick: u32) -> RepliconTick {
    let replicon_tick = RepliconTick::new(tick);
    app.world_mut()
        .resource_mut::<ReplicationCheckpointMap>()
        .record(replicon_tick, lightyear_core::prelude::Tick(tick));
    replicon_tick
}

/// Verifies that prediction buffers a newer patch range when its historical
/// base is missing, then materializes both the older and newer states once the
/// older base patch range arrives.
#[test]
fn diff_prediction_materializes_older_patch_after_newer_patch_arrives_first() {
    let (mut app, fns_id) = setup_prediction_receive_app();
    let tick0 = record_checkpoint(&mut app, 0);
    let tick3 = record_checkpoint(&mut app, 3);
    let tick5 = record_checkpoint(&mut app, 5);
    let entity = app.world_mut().spawn(Predicted).id();

    app.world_mut().entity_mut(entity).apply_write(
        diff_wire(DiffWire::Snapshot {
            cursor: Some(0),
            value: CompRepliconDiff(0),
        }),
        fns_id,
        tick0,
    );
    app.world_mut().entity_mut(entity).apply_write(
        diff_wire(DiffWire::Patches {
            first_patch_index: 4,
            patches: vec![vec![4], vec![5]],
        }),
        fns_id,
        tick5,
    );
    assert_eq!(
        app.world()
            .entity(entity)
            .get::<ConfirmedHistory<CompRepliconDiff>>()
            .unwrap()
            .get_state_at(lightyear_core::prelude::Tick(5)),
        None
    );

    app.world_mut().entity_mut(entity).apply_write(
        diff_wire(DiffWire::Patches {
            first_patch_index: 1,
            patches: vec![vec![1], vec![2], vec![3]],
        }),
        fns_id,
        tick3,
    );

    let history = app
        .world()
        .entity(entity)
        .get::<ConfirmedHistory<CompRepliconDiff>>()
        .unwrap();
    assert_eq!(
        history
            .get_state_at(lightyear_core::prelude::Tick(3))
            .and_then(ConfirmedState::value),
        Some(&CompRepliconDiff(3))
    );
    assert_eq!(
        history
            .get_state_at(lightyear_core::prelude::Tick(5))
            .and_then(ConfirmedState::value),
        Some(&CompRepliconDiff(5))
    );
}

/// Verifies that interpolation buffers a newer patch range when its historical
/// base is missing, then materializes both the older and newer states once the
/// older base patch range arrives.
#[test]
fn diff_interpolation_materializes_older_patch_after_newer_patch_arrives_first() {
    let (mut app, fns_id) = setup_interpolation_receive_app();
    let tick0 = record_checkpoint(&mut app, 0);
    let tick3 = record_checkpoint(&mut app, 3);
    let tick5 = record_checkpoint(&mut app, 5);
    let entity = app.world_mut().spawn(Interpolated).id();

    app.world_mut().entity_mut(entity).apply_write(
        diff_wire(DiffWire::Snapshot {
            cursor: Some(0),
            value: CompRepliconDiff(0),
        }),
        fns_id,
        tick0,
    );
    app.world_mut().entity_mut(entity).apply_write(
        diff_wire(DiffWire::Patches {
            first_patch_index: 4,
            patches: vec![vec![4], vec![5]],
        }),
        fns_id,
        tick5,
    );
    assert_eq!(
        app.world()
            .entity(entity)
            .get::<ConfirmedHistory<CompRepliconDiff>>()
            .unwrap()
            .get_state_at(lightyear_core::prelude::Tick(5)),
        None
    );

    app.world_mut().entity_mut(entity).apply_write(
        diff_wire(DiffWire::Patches {
            first_patch_index: 1,
            patches: vec![vec![1], vec![2], vec![3]],
        }),
        fns_id,
        tick3,
    );

    let history = app
        .world()
        .entity(entity)
        .get::<ConfirmedHistory<CompRepliconDiff>>()
        .unwrap();
    assert_eq!(
        history
            .get_state_at(lightyear_core::prelude::Tick(3))
            .and_then(ConfirmedState::value),
        Some(&CompRepliconDiff(3))
    );
    assert_eq!(
        history
            .get_state_at(lightyear_core::prelude::Tick(5))
            .and_then(ConfirmedState::value),
        Some(&CompRepliconDiff(5))
    );
}
