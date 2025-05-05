use crate::prelude::server::Replicate;
use crate::prelude::*;
use crate::shared::time_manager::WrappedTime;
use crate::tests::protocol::*;
use crate::tests::stepper::BevyStepper;
use bevy::prelude::*;
use core::time::Duration;

/// This test checks that replication still works if the client connect when the server
/// is on a new tick generation
#[test]
fn test_sync_after_tick_wrap() {
    let tick_duration = Duration::from_millis(10);
    let mut stepper = BevyStepper::default();

    // set time to end of wrapping
    let new_tick = Tick(u16::MAX - 100);
    let new_time = WrappedTime::from_duration(tick_duration * (new_tick.0 as u32));
    stepper
        .server_app
        .world_mut()
        .resource_mut::<TimeManager>()
        .set_current_time(new_time);
    stepper
        .server_app
        .world_mut()
        .resource_mut::<TickManager>()
        .set_tick_to(new_tick);

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((ComponentSyncModeFull(0.0), Replicate::default()))
        .id();

    // advance 200 ticks to wrap ticks around u16::MAX
    for i in 0..200 {
        stepper.frame_step();
    }
    // dbg!(&stepper.server_tick());
    // dbg!(&stepper.client_tick());
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(ComponentSyncModeFull(1.0));

    // make sure the client receives the replication message
    for i in 0..5 {
        stepper.frame_step();
    }

    let client_entity = stepper
        .client_app
        .world()
        .resource::<client::ConnectionManager>()
        .replication_receiver
        .remote_entity_map
        .get_local(server_entity)
        .unwrap();
    assert_eq!(
        stepper
            .client_app
            .world()
            .get::<ComponentSyncModeFull>(client_entity)
            .unwrap(),
        &ComponentSyncModeFull(1.0)
    );
}

/// This test checks that replication still works if the client connect when the server
/// is u16::MAX ticks ahead
#[test]
fn test_sync_after_tick_half_wrap() {
    let tick_duration = Duration::from_millis(10);
    let mut stepper = BevyStepper::default();

    // set time to end of wrapping
    let new_tick = Tick(u16::MAX / 2 - 10);
    let new_time = WrappedTime::from_duration(tick_duration * (new_tick.0 as u32));
    stepper
        .server_app
        .world_mut()
        .resource_mut::<TimeManager>()
        .set_current_time(new_time);
    stepper
        .server_app
        .world_mut()
        .resource_mut::<TickManager>()
        .set_tick_to(new_tick);

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((ComponentSyncModeFull(0.0), Replicate::default()))
        .id();

    for i in 0..200 {
        stepper.frame_step();
    }
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(ComponentSyncModeFull(1.0));
    // dbg!(&stepper.server_tick());
    // dbg!(&stepper.client_tick());
    let server_value = stepper
        .server_app
        .world()
        .get::<ComponentSyncModeFull>(server_entity)
        .unwrap();

    // make sure the client receives the replication message
    for i in 0..5 {
        stepper.frame_step();
    }

    let client_entity = stepper
        .client_app
        .world()
        .resource::<client::ConnectionManager>()
        .replication_receiver
        .remote_entity_map
        .get_local(server_entity)
        .unwrap();
    assert_eq!(
        stepper
            .client_app
            .world()
            .get::<ComponentSyncModeFull>(client_entity)
            .unwrap(),
        &ComponentSyncModeFull(1.0)
    );
}
