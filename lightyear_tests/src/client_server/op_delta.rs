use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::*;
use bevy_replicon::shared::replication::op_delta::{
    OpDeltaComponent, OpDeltaReceiver, OpDeltaWire, OpIndex, SequencedOp,
};
use bevy_replicon::shared::replication::registry::FnsId;
use bevy_replicon::shared::replication::registry::test_fns::TestFnsEntityExt;
use bevy_replicon::shared::replication::rules::ReplicationRules;
use lightyear_core::prelude::{Interpolated, Predicted, Tick};
use lightyear_interpolation::prelude::{ConfirmedHistory, InterpolationRegistrationExt};
use lightyear_prediction::prelude::{
    PredictionHistory, PredictionManager, PredictionRegistrationExt, PredictionRegistry,
    RollbackMode, RollbackPolicy, StateRollbackMetadata,
};
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::prelude::ComponentRegistry;
use lightyear_replication::registry::replication::ComponentRegistration;
use serde::{Deserialize, Serialize};

#[test]
fn op_delta_prediction_writes_snapshot_and_ops_to_prediction_history() {
    let mut app = setup_app();
    let fns_id = op_delta_fns_id(&app);
    let entity = app.world_mut().spawn((Predicted, OpDeltaCounter(999))).id();

    app.world_mut().entity_mut(entity).apply_write(
        snapshot(0, OpDeltaCounter(10)),
        fns_id,
        RepliconTick::new(1),
    );
    app.world_mut().entity_mut(entity).apply_write(
        ops(0, 2, [(1, CounterOp::Add(5)), (2, CounterOp::Add(7))]),
        fns_id,
        RepliconTick::new(2),
    );

    let entity_ref = app.world().entity(entity);
    assert_eq!(
        entity_ref.get::<OpDeltaCounter>(),
        Some(&OpDeltaCounter(999))
    );
    assert!(entity_ref.contains::<OpDeltaReceiver<OpDeltaCounter>>());

    let history = entity_ref
        .get::<PredictionHistory<OpDeltaCounter>>()
        .expect("op-delta predicted writes should populate PredictionHistory");
    assert_eq!(
        confirmed_prediction_value(history, Tick(10)),
        Some(OpDeltaCounter(10))
    );
    assert_eq!(
        confirmed_prediction_value(history, Tick(11)),
        Some(OpDeltaCounter(22))
    );
}

#[test]
fn op_delta_interpolation_writes_snapshot_and_ops_to_confirmed_history() {
    let mut app = setup_app();
    let fns_id = op_delta_fns_id(&app);
    let entity = app.world_mut().spawn(Interpolated).id();

    app.world_mut().entity_mut(entity).apply_write(
        snapshot(0, OpDeltaCounter(10)),
        fns_id,
        RepliconTick::new(1),
    );
    app.world_mut().entity_mut(entity).apply_write(
        ops(0, 2, [(1, CounterOp::Add(5)), (2, CounterOp::Add(7))]),
        fns_id,
        RepliconTick::new(2),
    );

    let entity_ref = app.world().entity(entity);
    assert!(!entity_ref.contains::<OpDeltaCounter>());
    assert!(entity_ref.contains::<OpDeltaReceiver<OpDeltaCounter>>());

    let history = entity_ref
        .get::<ConfirmedHistory<OpDeltaCounter>>()
        .expect("op-delta interpolated writes should populate ConfirmedHistory");
    assert_eq!(
        history_value(history.start()),
        Some((Tick(10), OpDeltaCounter(10)))
    );
    assert_eq!(
        history_value(history.end()),
        Some((Tick(11), OpDeltaCounter(22)))
    );
    assert_eq!(
        history_value(history.newest()),
        Some((Tick(11), OpDeltaCounter(22)))
    );
}

#[test]
fn op_delta_interpolation_replaces_writer_registered_by_interpolation_fn() {
    let mut app = setup_app_with_registration(register_op_delta_component_interpolation_fn_first);
    let fns_id = op_delta_fns_id(&app);
    let entity = app.world_mut().spawn(Interpolated).id();

    app.world_mut().entity_mut(entity).apply_write(
        snapshot(0, OpDeltaCounter(10)),
        fns_id,
        RepliconTick::new(1),
    );
    app.world_mut().entity_mut(entity).apply_write(
        ops(0, 2, [(1, CounterOp::Add(5)), (2, CounterOp::Add(7))]),
        fns_id,
        RepliconTick::new(2),
    );

    let entity_ref = app.world().entity(entity);
    assert!(!entity_ref.contains::<OpDeltaCounter>());
    assert!(entity_ref.contains::<OpDeltaReceiver<OpDeltaCounter>>());

    let history = entity_ref
        .get::<ConfirmedHistory<OpDeltaCounter>>()
        .expect("op-delta interpolated writes should populate ConfirmedHistory");
    assert_eq!(
        history_value(history.start()),
        Some((Tick(10), OpDeltaCounter(10)))
    );
    assert_eq!(
        history_value(history.end()),
        Some((Tick(11), OpDeltaCounter(22)))
    );
}

fn setup_app() -> App {
    setup_app_with_registration(register_op_delta_component)
}

fn setup_app_with_registration(register: impl FnOnce(&mut App)) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
    ));
    app.init_resource::<PredictionRegistry>();
    app.init_resource::<StateRollbackMetadata>();
    app.init_resource::<ReplicationCheckpointMap>();

    app.world_mut()
        .resource_mut::<ReplicationCheckpointMap>()
        .record(RepliconTick::new(1), Tick(10));
    app.world_mut()
        .resource_mut::<ReplicationCheckpointMap>()
        .record(RepliconTick::new(2), Tick(11));

    app.world_mut()
        .spawn(PredictionManager {
            rollback_policy: RollbackPolicy {
                state: RollbackMode::Disabled,
                input: RollbackMode::Disabled,
                ..Default::default()
            },
            ..Default::default()
        })
        .flush();

    register(&mut app);
    app.finish();
    app
}

fn register_op_delta_component(app: &mut App) {
    app.world_mut().init_resource::<ComponentRegistry>();
    app.world_mut()
        .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
            if !registry.is_registered::<OpDeltaCounter>() {
                registry.register_component::<OpDeltaCounter>(world);
            }
        });

    app.replicate_op_delta::<OpDeltaCounter>();
    ComponentRegistration::<OpDeltaCounter>::new(app)
        .add_prediction_op_delta()
        .add_custom_interpolation_op_delta();
}

fn register_op_delta_component_interpolation_fn_first(app: &mut App) {
    app.world_mut().init_resource::<ComponentRegistry>();
    app.world_mut()
        .resource_scope(|world, mut registry: Mut<ComponentRegistry>| {
            if !registry.is_registered::<OpDeltaCounter>() {
                registry.register_component::<OpDeltaCounter>(world);
            }
        });

    app.replicate_op_delta::<OpDeltaCounter>();
    ComponentRegistration::<OpDeltaCounter>::new(app)
        .add_prediction_op_delta()
        .register_interpolation_fn(interpolate_counter)
        .add_custom_interpolation_op_delta();
}

fn confirmed_prediction_value(
    history: &PredictionHistory<OpDeltaCounter>,
    tick: Tick,
) -> Option<OpDeltaCounter> {
    history
        .get_confirmed_at(tick)
        .and_then(|state| state.value())
        .cloned()
}

fn history_value(value: Option<(Tick, &OpDeltaCounter)>) -> Option<(Tick, OpDeltaCounter)> {
    value.map(|(tick, value)| (tick, value.clone()))
}

fn op_delta_fns_id(app: &App) -> FnsId {
    app.world().resource::<ReplicationRules>()[0].components[0].fns_id
}

fn snapshot(cursor: OpIndex, value: OpDeltaCounter) -> Vec<u8> {
    wire(OpDeltaWire::Snapshot { cursor, value })
}

fn ops<const N: usize>(
    base_cursor: OpIndex,
    cursor: OpIndex,
    ops: [(OpIndex, CounterOp); N],
) -> Vec<u8> {
    wire(OpDeltaWire::Ops {
        base_cursor,
        cursor,
        ops: ops
            .into_iter()
            .map(|(seq, op)| SequencedOp { seq, op })
            .collect(),
    })
}

fn wire(wire: OpDeltaWire<OpDeltaCounter, CounterOp>) -> Vec<u8> {
    let mut message = Vec::new();
    postcard_utils::to_extend_mut(&wire, &mut message).unwrap();
    message
}

fn interpolate_counter(start: OpDeltaCounter, end: OpDeltaCounter, t: f32) -> OpDeltaCounter {
    OpDeltaCounter(((1.0 - t) * start.0 as f32 + t * end.0 as f32).round() as i32)
}

#[derive(Component, Clone, Debug, Deserialize, PartialEq, Serialize)]
struct OpDeltaCounter(i32);

impl OpDeltaComponent for OpDeltaCounter {
    type Op = CounterOp;

    fn apply_op(&mut self, op: &Self::Op) -> bevy::ecs::error::Result<()> {
        match *op {
            CounterOp::Add(value) => self.0 += value,
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum CounterOp {
    Add(i32),
}
