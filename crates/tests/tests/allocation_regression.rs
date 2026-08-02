use std::alloc::System;
use std::sync::Mutex;

use bevy::ecs::schedule::{Schedules, SingleThreadedExecutor};
use bevy::prelude::{Entity, FixedUpdate, Query, Res, ResMut, Resource, Update, With};
use lightyear::prelude::{
    MessageReceiver, MessageSender, NetworkTarget, Predicted, PredictionTarget, Replicate,
    Transport,
};
use lightyear_messages::MessageManager;
use lightyear_prediction::predicted_history::PredictionHistory;
use lightyear_tests::protocol::{Channel1, CompFull, StringMessage};
use lightyear_tests::stepper::{ClientServerStepper, ClientType, ServerType, StepperConfig};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
static ALLOCATION_TEST_LOCK: Mutex<()> = Mutex::new(());

const WARMUP_FRAMES: usize = 128;
const MEASURED_FRAMES: usize = 32;

#[derive(Resource, Default)]
struct ReceivedMessageCount(usize);

#[derive(Resource)]
struct SimulationEnabled(bool);

#[derive(Debug)]
struct AllocationMeasurement {
    idle: Stats,
    active: Stats,
}

#[derive(Debug)]
struct AllocationBudget {
    max_allocations_per_frame: usize,
    max_bytes_per_frame: usize,
}

#[test]
fn steady_state_networking_work_stays_within_allocation_budget() {
    let _guard = ALLOCATION_TEST_LOCK.lock().unwrap();
    let message_stats = measure_message_send_receive();
    let replication_stats = measure_replication_updates();
    let prediction_stats = measure_prediction_updates();

    eprintln!("message send/receive steady-state allocations: {message_stats:#?}");
    eprintln!("replication update steady-state allocations: {replication_stats:#?}");
    eprintln!("prediction update steady-state allocations: {prediction_stats:#?}");

    // These are deliberately coarse per-frame ceilings. They detect meaningful
    // regressions without depending on allocator layouts or upstream struct sizes.
    assert_allocation_budget(
        "message send/receive",
        message_stats,
        AllocationBudget {
            max_allocations_per_frame: 16,
            max_bytes_per_frame: 2 * 1024,
        },
    );
    let entity_update_budget = || AllocationBudget {
        max_allocations_per_frame: 2,
        max_bytes_per_frame: 1024,
    };
    assert_allocation_budget(
        "replication update",
        replication_stats,
        entity_update_budget(),
    );
    assert_allocation_budget(
        "prediction update",
        prediction_stats,
        entity_update_budget(),
    );
}

#[test]
fn packet_payload_pool_has_no_misses_after_warmup_through_crossbeam_io() {
    let _guard = ALLOCATION_TEST_LOCK.lock().unwrap();
    let mut stepper = ClientServerStepper::from_config(StepperConfig::from_link_types(
        vec![ClientType::Raw],
        ServerType::Raw,
    ));
    stepper.server_app.init_resource::<ReceivedMessageCount>();
    stepper
        .server_app
        .add_systems(Update, count_received_messages);
    stepper.client_app().init_resource::<ReceivedMessageCount>();
    stepper
        .client_app()
        .add_systems(Update, count_received_messages);
    use_single_threaded_schedules(&mut stepper);

    for _ in 0..WARMUP_FRAMES {
        run_bidirectional_message_cycle(&mut stepper);
    }
    let misses_before = packet_payload_pool_misses(&stepper);
    assert!(
        misses_before > 0,
        "warmup should exercise packet payload allocation instrumentation",
    );

    for _ in 0..MEASURED_FRAMES {
        run_bidirectional_message_cycle(&mut stepper);
    }

    assert_eq!(
        packet_payload_pool_misses(&stepper),
        misses_before,
        "the real Transport -> Link -> Crossbeam IO path allocated a packet payload after warmup",
    );
}

fn packet_payload_pool_misses(stepper: &ClientServerStepper) -> usize {
    let client_misses = stepper
        .client(0)
        .get::<Transport>()
        .expect("client should have a Transport")
        .packet_payload_pool_misses();
    let server_misses = stepper
        .client_of(0)
        .get::<Transport>()
        .expect("server-side client should have a Transport")
        .packet_payload_pool_misses();
    client_misses + server_misses
}

fn measure_message_send_receive() -> AllocationMeasurement {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.server_app.init_resource::<ReceivedMessageCount>();
    stepper
        .server_app
        .add_systems(Update, count_received_messages);
    stepper.client_app().init_resource::<ReceivedMessageCount>();
    stepper
        .client_app()
        .add_systems(Update, count_received_messages);
    use_single_threaded_schedules(&mut stepper);

    for _ in 0..WARMUP_FRAMES {
        run_bidirectional_message_cycle(&mut stepper);
    }

    for _ in 0..WARMUP_FRAMES {
        run_idle_message_cycle(&mut stepper);
    }
    let region = Region::new(GLOBAL);
    for _ in 0..MEASURED_FRAMES {
        run_idle_message_cycle(&mut stepper);
    }
    let idle = region.change();

    for _ in 0..WARMUP_FRAMES {
        run_bidirectional_message_cycle(&mut stepper);
    }
    let region = Region::new(GLOBAL);
    for _ in 0..MEASURED_FRAMES {
        run_bidirectional_message_cycle(&mut stepper);
    }
    AllocationMeasurement {
        idle,
        active: region.change(),
    }
}

fn run_idle_message_cycle(stepper: &mut ClientServerStepper) {
    stepper.frame_step_server_first(1);
    stepper.frame_step(1);
}

fn run_bidirectional_message_cycle(stepper: &mut ClientServerStepper) {
    let client_received_before = stepper
        .client_app()
        .world()
        .resource::<ReceivedMessageCount>()
        .0;
    stepper
        .client_of_mut(0)
        .get_mut::<MessageSender<StringMessage>>()
        .expect("server-side message sender should exist")
        .send::<Channel1>(StringMessage(String::new()));
    stepper.frame_step_server_first(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .resource::<ReceivedMessageCount>()
            .0,
        client_received_before + 1,
    );

    let server_received_before = stepper
        .server_app
        .world()
        .resource::<ReceivedMessageCount>()
        .0;
    stepper
        .client_mut(0)
        .get_mut::<MessageSender<StringMessage>>()
        .expect("client-side message sender should exist")
        .send::<Channel1>(StringMessage(String::new()));
    stepper.frame_step(1);
    assert_eq!(
        stepper
            .server_app
            .world()
            .resource::<ReceivedMessageCount>()
            .0,
        server_received_before + 1,
    );
}

fn count_received_messages(
    mut receivers: Query<&mut MessageReceiver<StringMessage>>,
    mut count: ResMut<ReceivedMessageCount>,
) {
    for mut receiver in &mut receivers {
        count.0 += receiver.receive().count();
    }
}

fn measure_replication_updates() -> AllocationMeasurement {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    use_single_threaded_schedules(&mut stepper);
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((CompFull(0.0), Replicate::to_clients(NetworkTarget::All)))
        .id();
    let client_entity = wait_for_mapped_client_entity(&mut stepper, server_entity);

    for _ in 0..WARMUP_FRAMES {
        run_replication_update(&mut stepper, server_entity, client_entity);
    }

    for _ in 0..WARMUP_FRAMES {
        stepper.frame_step_server_first(1);
    }
    let region = Region::new(GLOBAL);
    for _ in 0..MEASURED_FRAMES {
        stepper.frame_step_server_first(1);
    }
    let idle = region.change();

    for _ in 0..WARMUP_FRAMES {
        run_replication_update(&mut stepper, server_entity, client_entity);
    }
    let region = Region::new(GLOBAL);
    for _ in 0..MEASURED_FRAMES {
        run_replication_update(&mut stepper, server_entity, client_entity);
    }
    AllocationMeasurement {
        idle,
        active: region.change(),
    }
}

fn run_replication_update(
    stepper: &mut ClientServerStepper,
    server_entity: Entity,
    client_entity: Entity,
) {
    let expected = {
        let mut component = stepper
            .server_app
            .world_mut()
            .get_mut::<CompFull>(server_entity)
            .expect("server component should exist");
        component.0 += 1.0;
        component.0
    };
    stepper.frame_step_server_first(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(client_entity)
            .expect("replicated client component should exist")
            .0,
        expected,
    );
}

fn measure_prediction_updates() -> AllocationMeasurement {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.server_app.insert_resource(SimulationEnabled(true));
    stepper
        .client_app()
        .insert_resource(SimulationEnabled(true));
    stepper
        .server_app
        .add_systems(FixedUpdate, increment_component);
    stepper
        .client_app()
        .add_systems(FixedUpdate, increment_predicted_component);
    use_single_threaded_schedules(&mut stepper);

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            CompFull(0.0),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    let client_entity = wait_for_mapped_client_entity(&mut stepper, server_entity);
    assert!(
        stepper
            .client_app()
            .world()
            .get::<Predicted>(client_entity)
            .is_some(),
        "client entity should be predicted",
    );
    assert!(
        stepper
            .client_app()
            .world()
            .get::<PredictionHistory<CompFull>>(client_entity)
            .is_some(),
        "predicted component should have a history buffer",
    );

    for _ in 0..WARMUP_FRAMES {
        run_prediction_update(&mut stepper);
    }

    stepper
        .server_app
        .world_mut()
        .resource_mut::<SimulationEnabled>()
        .0 = false;
    stepper
        .client_app()
        .world_mut()
        .resource_mut::<SimulationEnabled>()
        .0 = false;
    for _ in 0..WARMUP_FRAMES {
        run_prediction_update(&mut stepper);
    }
    let region = Region::new(GLOBAL);
    for _ in 0..MEASURED_FRAMES {
        run_prediction_update(&mut stepper);
    }
    let idle = region.change();

    stepper
        .server_app
        .world_mut()
        .resource_mut::<SimulationEnabled>()
        .0 = true;
    stepper
        .client_app()
        .world_mut()
        .resource_mut::<SimulationEnabled>()
        .0 = true;
    for _ in 0..WARMUP_FRAMES {
        run_prediction_update(&mut stepper);
    }
    let region = Region::new(GLOBAL);
    for _ in 0..MEASURED_FRAMES {
        run_prediction_update(&mut stepper);
    }
    let active = region.change();

    let predicted = stepper
        .client_app()
        .world()
        .get::<CompFull>(client_entity)
        .expect("predicted client component should exist")
        .0;
    let authoritative = stepper
        .server_app
        .world()
        .get::<CompFull>(server_entity)
        .expect("authoritative server component should exist")
        .0;
    assert!(
        predicted > authoritative,
        "prediction should simulate ahead of the authoritative server",
    );
    assert!(
        !stepper
            .client_app()
            .world()
            .get::<PredictionHistory<CompFull>>(client_entity)
            .expect("predicted component should retain its history buffer")
            .is_empty(),
        "prediction should record component history",
    );
    AllocationMeasurement { idle, active }
}

fn increment_component(enabled: Res<SimulationEnabled>, mut components: Query<&mut CompFull>) {
    if !enabled.0 {
        return;
    }
    for mut component in &mut components {
        component.0 += 1.0;
    }
}

fn increment_predicted_component(
    enabled: Res<SimulationEnabled>,
    mut components: Query<&mut CompFull, With<Predicted>>,
) {
    if !enabled.0 {
        return;
    }
    for mut component in &mut components {
        component.0 += 1.0;
    }
}

fn run_prediction_update(stepper: &mut ClientServerStepper) {
    stepper.frame_step_server_first(1);
}

fn mapped_client_entity(stepper: &ClientServerStepper, server_entity: Entity) -> Entity {
    stepper
        .client(0)
        .get::<MessageManager>()
        .expect("client message manager should exist")
        .entity_mapper
        .get_local(server_entity)
        .expect("server entity should be mapped on the client")
}

fn wait_for_mapped_client_entity(
    stepper: &mut ClientServerStepper,
    server_entity: Entity,
) -> Entity {
    for _ in 0..50 {
        stepper.frame_step_server_first(1);
        if let Some(client_entity) = stepper
            .client(0)
            .get::<MessageManager>()
            .expect("client message manager should exist")
            .entity_mapper
            .get_local(server_entity)
        {
            return client_entity;
        }
    }
    mapped_client_entity(stepper, server_entity)
}

fn use_single_threaded_schedules(stepper: &mut ClientServerStepper) {
    for app in core::iter::once(&mut stepper.server_app).chain(stepper.client_apps.iter_mut()) {
        for (_, schedule) in app.world_mut().resource_mut::<Schedules>().iter_mut() {
            schedule.set_executor(SingleThreadedExecutor::new());
        }
    }
}

fn assert_allocation_budget(
    name: &str,
    measurement: AllocationMeasurement,
    budget: AllocationBudget,
) {
    let max_allocations = MEASURED_FRAMES * budget.max_allocations_per_frame;
    let max_bytes = MEASURED_FRAMES * budget.max_bytes_per_frame;
    let incremental_allocations = measurement
        .active
        .allocations
        .saturating_sub(measurement.idle.allocations);
    let incremental_bytes = measurement
        .active
        .bytes_allocated
        .saturating_sub(measurement.idle.bytes_allocated);

    assert!(
        incremental_allocations <= max_allocations,
        "{name} exceeded its allocation-call budget ({incremental_allocations} > {}): \
         {measurement:#?}",
        max_allocations,
    );
    assert!(
        measurement.active.reallocations <= measurement.idle.reallocations,
        "{name} added reallocation calls beyond the idle pipeline: {measurement:#?}",
    );
    assert!(
        incremental_bytes <= max_bytes,
        "{name} exceeded its allocated-byte budget ({incremental_bytes} > {}): {measurement:#?}",
        max_bytes,
    );
    assert_eq!(
        measurement.active.allocations, measurement.active.deallocations,
        "{name} retained allocations after the measured steady-state window: {measurement:#?}",
    );
    assert_eq!(
        measurement.active.bytes_allocated, measurement.active.bytes_deallocated,
        "{name} retained allocated bytes after the measured steady-state window: {measurement:#?}",
    );
}
