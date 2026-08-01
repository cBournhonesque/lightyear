use bevy::ecs::schedule::{Schedules, SingleThreadedExecutor};
use bevy::prelude::{Entity, FixedUpdate, Query, With};
use lightyear::prelude::{MessageSender, NetworkTarget, Predicted, PredictionTarget, Replicate};
use lightyear_messages::MessageManager;
use lightyear_tests::protocol::{Channel1, CompFull, StringMessage};
use lightyear_tests::stepper::{ClientServerStepper, StepperConfig};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[test]
#[ignore = "manual heap profile; writes target/dhat-network-idle-frame.json"]
fn dhat_warmed_idle_network_frame() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    use_single_threaded_schedules(&mut stepper);
    for _ in 0..128 {
        stepper.frame_step_server_first(1);
    }

    let profile_path = profile_path("idle-frame");
    let _profiler = dhat::Profiler::builder().file_name(&profile_path).build();
    stepper.frame_step_server_first(1);
}

#[test]
#[ignore = "manual heap profile; writes target/dhat-network-messages.json"]
fn dhat_warmed_bidirectional_messages() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    use_single_threaded_schedules(&mut stepper);
    for _ in 0..128 {
        run_bidirectional_message_cycle(&mut stepper);
    }

    let profile_path = profile_path("messages");
    let _profiler = dhat::Profiler::builder().file_name(&profile_path).build();
    run_bidirectional_message_cycle(&mut stepper);
}

#[test]
#[ignore = "manual heap profile; writes target/dhat-network-replication.json"]
fn dhat_warmed_replication_update() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    use_single_threaded_schedules(&mut stepper);
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((CompFull(0.0), Replicate::to_clients(NetworkTarget::All)))
        .id();
    let client_entity = wait_for_mapped_client_entity(&mut stepper, server_entity);
    for _ in 0..128 {
        run_replication_update(&mut stepper, server_entity, client_entity);
    }

    let profile_path = profile_path("replication");
    let _profiler = dhat::Profiler::builder().file_name(&profile_path).build();
    run_replication_update(&mut stepper, server_entity, client_entity);
}

#[test]
#[ignore = "manual heap profile; writes target/dhat-network-prediction.json"]
fn dhat_warmed_prediction_update() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
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
    wait_for_mapped_client_entity(&mut stepper, server_entity);
    for _ in 0..128 {
        stepper.frame_step_server_first(1);
    }

    let profile_path = profile_path("prediction");
    let _profiler = dhat::Profiler::builder().file_name(&profile_path).build();
    stepper.frame_step_server_first(1);
}

fn use_single_threaded_schedules(stepper: &mut ClientServerStepper) {
    for app in core::iter::once(&mut stepper.server_app).chain(stepper.client_apps.iter_mut()) {
        for (_, schedule) in app.world_mut().resource_mut::<Schedules>().iter_mut() {
            schedule.set_executor(SingleThreadedExecutor::new());
        }
    }
}

fn run_bidirectional_message_cycle(stepper: &mut ClientServerStepper) {
    stepper
        .client_of_mut(0)
        .get_mut::<MessageSender<StringMessage>>()
        .unwrap()
        .send::<Channel1>(StringMessage(String::new()));
    stepper.frame_step_server_first(1);
    stepper
        .client_mut(0)
        .get_mut::<MessageSender<StringMessage>>()
        .unwrap()
        .send::<Channel1>(StringMessage(String::new()));
    stepper.frame_step(1);
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
            .unwrap();
        component.0 += 1.0;
        component.0
    };
    stepper.frame_step_server_first(1);
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<CompFull>(client_entity)
            .unwrap()
            .0,
        expected,
    );
}

fn increment_component(mut components: Query<&mut CompFull>) {
    for mut component in &mut components {
        component.0 += 1.0;
    }
}

fn increment_predicted_component(mut components: Query<&mut CompFull, With<Predicted>>) {
    for mut component in &mut components {
        component.0 += 1.0;
    }
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
            .unwrap()
            .entity_mapper
            .get_local(server_entity)
        {
            return client_entity;
        }
    }
    panic!("server entity should be mapped on the client");
}

fn profile_path(name: &str) -> std::path::PathBuf {
    let profile_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/dhat-network-{name}.json"));
    std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
    profile_path
}
