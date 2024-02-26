use crate::_reexport::WrappedTime;
use crate::prelude::client::{InputSystemSet, SyncConfig};
use crate::prelude::server::InputEvent;
use crate::prelude::*;
use crate::tests::protocol::*;
use crate::tests::stepper::{BevyStepper, Step};
use bevy::prelude::*;
use bevy::utils::Duration;

fn press_input(mut connection: ResMut<ClientConnectionManager>, tick_manager: Res<TickManager>) {
    connection.add_input(MyInput(0), tick_manager.tick());
}
fn increment(mut query: Query<&mut Component1>, mut ev: EventReader<InputEvent<MyInput>>) {
    for _ in ev.read() {
        for mut c in query.iter_mut() {
            c.0 += 1.0;
        }
    }
}

/// This test checks that input handling and replication still works if the client connect when the server
/// is on a new tick generation
#[test]
fn test_sync_after_tick_wrap() {
    let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
    let tick_duration = Duration::from_millis(10);
    let shared_config = SharedConfig {
        tick: TickConfig::new(tick_duration),
        ..Default::default()
    };
    let link_conditioner = LinkConditionerConfig {
        incoming_latency: Duration::from_millis(20),
        incoming_jitter: Duration::from_millis(0),
        incoming_loss: 0.0,
    };
    let mut stepper = BevyStepper::new(
        shared_config,
        SyncConfig::default(),
        client::PredictionConfig::default(),
        client::InterpolationConfig::default(),
        link_conditioner,
        frame_duration,
    );

    // set time to end of wrapping
    let new_tick = Tick(u16::MAX - 100);
    let new_time = WrappedTime::from_duration(tick_duration * (new_tick.0 as u32));
    stepper
        .server_app
        .world
        .resource_mut::<TimeManager>()
        .set_current_time(new_time);
    stepper
        .server_app
        .world
        .resource_mut::<TickManager>()
        .set_tick_to(new_tick);

    stepper.client_app.add_systems(
        FixedPreUpdate,
        press_input.in_set(InputSystemSet::BufferInputs),
    );
    stepper.server_app.add_systems(FixedUpdate, increment);

    let server_entity = stepper
        .server_app
        .world
        .spawn((
            Component1(0.0),
            Replicate {
                replication_target: NetworkTarget::All,
                ..default()
            },
        ))
        .id();

    for i in 0..200 {
        stepper.frame_step();
    }
    stepper.init();
    dbg!(&stepper.server_tick());
    dbg!(&stepper.client_tick());
    let server_value = stepper
        .server_app
        .world
        .get::<Component1>(server_entity)
        .unwrap();

    // make sure the client receives the replication message
    for i in 0..5 {
        stepper.frame_step();
    }

    let client_entity = *stepper
        .client_app
        .world
        .resource::<ClientConnectionManager>()
        .replication_receiver
        .remote_entity_map
        .get_local(server_entity)
        .unwrap();
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Component1>(client_entity)
            .unwrap(),
        &Component1(47.0)
    );
}

/// This test checks that input handling and replication still works if the client connect when the server
/// is u16::MAX ticks ahead
#[test]
fn test_sync_after_tick_half_wrap() {
    let frame_duration = Duration::from_secs_f32(1.0 / 60.0);
    let tick_duration = Duration::from_millis(10);
    let shared_config = SharedConfig {
        tick: TickConfig::new(tick_duration),
        ..Default::default()
    };
    let link_conditioner = LinkConditionerConfig {
        incoming_latency: Duration::from_millis(20),
        incoming_jitter: Duration::from_millis(0),
        incoming_loss: 0.0,
    };
    let mut stepper = BevyStepper::new(
        shared_config,
        SyncConfig::default(),
        client::PredictionConfig::default(),
        client::InterpolationConfig::default(),
        link_conditioner,
        frame_duration,
    );

    // set time to end of wrapping
    let new_tick = Tick(u16::MAX / 2 - 10);
    let new_time = WrappedTime::from_duration(tick_duration * (new_tick.0 as u32));
    stepper
        .server_app
        .world
        .resource_mut::<TimeManager>()
        .set_current_time(new_time);
    stepper
        .server_app
        .world
        .resource_mut::<TickManager>()
        .set_tick_to(new_tick);

    stepper.client_app.add_systems(
        FixedPreUpdate,
        press_input.in_set(InputSystemSet::BufferInputs),
    );
    stepper.server_app.add_systems(FixedUpdate, increment);

    let server_entity = stepper
        .server_app
        .world
        .spawn((
            Component1(0.0),
            Replicate {
                replication_target: NetworkTarget::All,
                ..default()
            },
        ))
        .id();

    for i in 0..200 {
        stepper.frame_step();
    }
    stepper.init();
    dbg!(&stepper.server_tick());
    dbg!(&stepper.client_tick());
    let server_value = stepper
        .server_app
        .world
        .get::<Component1>(server_entity)
        .unwrap();

    // make sure the client receives the replication message
    for i in 0..5 {
        stepper.frame_step();
    }

    let client_entity = *stepper
        .client_app
        .world
        .resource::<ClientConnectionManager>()
        .replication_receiver
        .remote_entity_map
        .get_local(server_entity)
        .unwrap();
    assert_eq!(
        stepper
            .client_app
            .world
            .get::<Component1>(client_entity)
            .unwrap(),
        &Component1(47.0)
    );
}
