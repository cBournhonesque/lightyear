//! This test is to ensure that the physics engine is deterministic.
//!
//! Run with `cargo test avian_determinism --features avian2d,avian2d/2d,avian2d/f32,avian2d/parry-f32`
use crate::prelude::client::ClientConfig;
use crate::prelude::{SharedConfig, TickConfig, TickManager};
use crate::tests::stepper::{BevyStepper, Step};
use avian2d::prelude::*;
use bevy::prelude::*;
use std::time::Duration;

fn positions(query: Query<(Entity, &Position)>, tick_manager: Res<TickManager>) {
    let tick = tick_manager.tick();
    info!(?tick);
    for (entity, position) in query.iter() {
        info!(?entity, ?position);
    }
}

#[test]
fn test_avian_determinism() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .init();
    let frame_duration = Duration::from_millis(10);
    let tick_duration = Duration::from_millis(10);
    let shared_config = SharedConfig {
        tick: TickConfig::new(tick_duration),
        ..Default::default()
    };
    let client_config = ClientConfig::default();

    let mut stepper = BevyStepper::new(shared_config, client_config, frame_duration);
    stepper
        .client_app
        .add_plugins(PhysicsPlugins::new(FixedUpdate))
        .insert_resource(Time::new_with(Physics::fixed_once_hz(100.0)))
        .insert_resource(Gravity(Vec2::ZERO));
    stepper.client_app.add_systems(Last, positions);
    stepper.init();

    let square = stepper
        .client_app
        .world_mut()
        .spawn((
            Position(Vec2::new(0.0, 0.0)),
            LinearVelocity(Vec2::new(0.0, 20.0)),
            Collider::rectangle(1.0, 1.0),
            ColliderDensity(0.2),
            RigidBody::Dynamic,
        ))
        .id();
    let ball = stepper
        .client_app
        .world_mut()
        .spawn((
            Position(Vec2::new(0.0, 10.0)),
            LinearVelocity(Vec2::new(0.0, -2.0)),
            Collider::circle(1.0),
            ColliderDensity(0.05),
            RigidBody::Dynamic,
        ))
        .id();

    let tick = stepper.client_tick();
    info!("Start");
    for _ in 0..100 {
        stepper.frame_step();
    }
}
