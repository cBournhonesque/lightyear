use crate::protocol::*;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy_screen_diagnostics::{Aggregate, ScreenDiagnostics, ScreenDiagnosticsPlugin};
use bevy_xpbd_3d::parry::shape::Ball;
use bevy_xpbd_3d::prelude::*;
use bevy_xpbd_3d::{PhysicsSchedule, PhysicsStepSet};
use leafwing_input_manager::prelude::ActionState;
use lightyear::client::prediction::{Rollback, RollbackState};
use lightyear::prelude::client::*;
use lightyear::prelude::TickManager;
use lightyear::prelude::*;
use lightyear::transport::io::IoDiagnosticsPlugin;
use std::time::Duration;
use tracing::Level;

const FRAME_HZ: f64 = 60.0;
const FIXED_TIMESTEP_HZ: f64 = 64.0;
const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;

pub fn shared_config() -> SharedConfig {
    SharedConfig {
        enable_replication: true,
        client_send_interval: Duration::default(),
        server_send_interval: Duration::from_secs_f64(1.0 / 32.0),
        // server_send_interval: Duration::from_millis(100),
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        },
        log: LogConfig {
            level: Level::WARN,
            filter: "wgpu=error,wgpu_hal=error,naga=warn,bevy_app=info,bevy_render=warn,quinn=warn"
                .to_string(),
        }
    }
}

#[derive(Component)]
pub struct MeshShape {
    pub shape: Mesh,
    pub color: Color,
}

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        if app.is_plugin_added::<RenderPlugin>() {
            // limit frame rate
            // app.add_plugins(bevy_framepace::FramepacePlugin);
            // app.world
            //     .resource_mut::<bevy_framepace::FramepaceSettings>()
            //     .limiter = bevy_framepace::Limiter::from_framerate(FRAME_HZ);

            // show framerate
            // use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
            // app.add_plugins(FrameTimeDiagnosticsPlugin::default());
            // app.add_plugins(bevy_fps_counter::FpsCounterPlugin);

            // draw after interpolation is done
            /*app.add_systems(
                PostUpdate,
                draw_elements
                    .after(InterpolationSet::Interpolate)
                    .after(PredictionSet::VisualCorrection),
            );*/
            app.add_plugins(LogDiagnosticsPlugin {
                filter: Some(vec![
                    IoDiagnosticsPlugin::BYTES_IN,
                    IoDiagnosticsPlugin::BYTES_OUT,
                ]),
                ..default()
            });
            app.add_systems(Startup, setup_diagnostic);
            app.add_plugins(ScreenDiagnosticsPlugin::default());
        }
        // bundles
        app.add_systems(Startup, init);

        if app.is_plugin_added::<RenderPlugin>() {
            // only run *add_meshes_for_rendering* if we have the renderplugin added
            // this avoid issues when running headless
            app.add_systems(Update, add_meshes_for_rendering);
        }

        // physics
        app.add_plugins(PhysicsPlugins::new(FixedUpdate))
            .insert_resource(Time::new_with(Physics::fixed_once_hz(FIXED_TIMESTEP_HZ)))
            //.insert_resource(Gravity(Vec3::ZERO));
            .insert_resource(Gravity(Vec3::new(0.0, -10.0, 0.0)));
        app.configure_sets(
            FixedUpdate,
            // make sure that any physics simulation happens after the Main SystemSet
            // (where we apply user's actions)
            (
                PhysicsSet::Prepare,
                PhysicsSet::StepSimulation,
                PhysicsSet::Sync,
            )
                .in_set(FixedUpdateSet::Main),
        );
        // add a log at the start of the physics schedule
        app.add_systems(PhysicsSchedule, log.in_set(PhysicsStepSet::BroadPhase));

        app.add_systems(
            FixedUpdate,
            after_physics_log.after(FixedUpdateSet::Main),
        );
        app.add_systems(Last, last_log);

        // registry types for reflection
        app.register_type::<PlayerId>();
    }
}

fn setup_diagnostic(mut onscreen: ResMut<ScreenDiagnostics>) {
    onscreen
        .add("bytes_in".to_string(), IoDiagnosticsPlugin::BYTES_IN)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.0}"));
    onscreen
        .add("bytes_out".to_string(), IoDiagnosticsPlugin::BYTES_OUT)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.0}"));
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = ((client_id * 90) % 360) as f32;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

const WALL_HEIGHT:f32 = 10.0;
pub(crate) fn init(
    mut commands: Commands,
) {
    commands.spawn((
        PointLightBundle {
            transform: Transform::from_translation(Vec3::new(-2.0, 2.0, -2.0)),
            point_light: PointLight {
                color: Color::WHITE,
                intensity: 1500.0,
                shadows_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        },
    ));

    // left wall
    commands.spawn((
        MeshShape {
            shape: shape::Box::new(
                0.10,
                WALL_HEIGHT,
                10.0,
            ).into(),
            color: Color::rgb_u8(124, 144, 255).into(),
        },
        TransformBundle {
            local: Transform::from_translation(
                Vec3::new(-5.0, WALL_HEIGHT * 0.5, 0.0)
            ),
            ..Default::default()
        },
        PhysicsBundle {
            collider: Collider::cuboid(0.10, WALL_HEIGHT, 10.0),
            collider_density: ColliderDensity(1.0),
            rigid_body: RigidBody::Static,
        },
    ));
    // right wall
    commands.spawn((
        MeshShape {
            shape: shape::Box::new(
                0.10,
                WALL_HEIGHT,
                10.0,
            ).into(),
            color: Color::rgb_u8(124, 144, 255).into(),
        },
        TransformBundle {
            local: Transform::from_translation(
                Vec3::new(5.0, WALL_HEIGHT * 0.5, 0.0)
            ),
            ..Default::default()
        },
        PhysicsBundle {
            collider: Collider::cuboid(0.10, WALL_HEIGHT, 10.0),
            collider_density: ColliderDensity(1.0),
            rigid_body: RigidBody::Static,
        },
    ));
    // top wall
    commands.spawn((
        MeshShape {
            shape: shape::Box::new(
                0.10,
                WALL_HEIGHT,
                10.0,
            ).into(),
            color: Color::rgb_u8(124, 144, 255).into(),
        },
        TransformBundle {
            local: Transform::from_translation(
                Vec3::new(0.0, WALL_HEIGHT * 0.5, 5.0)
            ).with_rotation(Quat::from_rotation_y(90.0_f32.to_radians())),
            ..Default::default()
        },
        PhysicsBundle {
            collider: Collider::cuboid(0.10, WALL_HEIGHT, 10.0),
            collider_density: ColliderDensity(1.0),
            rigid_body: RigidBody::Static,
        },
    ));
    // bottom wall
    commands.spawn((
        MeshShape {
            shape: shape::Box::new(
                0.10,
                WALL_HEIGHT,
                10.0,
            ).into(),
            color: Color::rgb_u8(124, 144, 255).into(),
        },
        TransformBundle {
            local: Transform::from_translation(
                Vec3::new(0.0, WALL_HEIGHT * 0.5, -5.0)
            ).with_rotation(Quat::from_rotation_y(90.0_f32.to_radians())),
            ..Default::default()
        },
        PhysicsBundle {
            collider: Collider::cuboid(0.10, WALL_HEIGHT, 10.0),
            collider_density: ColliderDensity(1.0),
            rigid_body: RigidBody::Static,
        },
    ));

    commands.spawn((
        MeshShape {
            shape: shape::Plane { size: 10.0, subdivisions: 10 }.into(),
            color: Color::rgb_u8(0x7c, 0x80, 0x76),
        },
        TransformBundle {
            local: Transform::from_translation(
                Vec3::ZERO
            ),
            ..Default::default()
        },
        PhysicsBundle {
            collider: Collider::cuboid(10.0, 0.1, 100.0),
            collider_density: ColliderDensity(1.0),
            rigid_body: RigidBody::Static,
        },
    ));
}

// This system looks for entities with a MeshShape component and adds a PbrBundle to them
//
pub(crate) fn add_meshes_for_rendering(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    query: Query<(Entity, &MeshShape), (Added<MeshShape>, Without<Handle<Mesh>>)>,
) {

    for (entity, mesh_shape) in query.iter() {
        commands.entity(entity).insert((
            meshes.add(mesh_shape.shape.clone()),
            materials.add(mesh_shape.color.into()),
            VisibilityBundle {
                ..Default::default()
            },
        ));
    }
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(
    mut velocity: Mut<LinearVelocity>,
    action: &ActionState<PlayerActions>,
) {
    const MOVE_SPEED: f32 = 1.0;
    if action.pressed(PlayerActions::Up) {
        velocity.z -= MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Down) {
        velocity.z += MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Left) {
        velocity.x -= MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Right) {
        velocity.x += MOVE_SPEED;
    }
    *velocity = LinearVelocity(velocity.clamp_length_max(MAX_VELOCITY));
}

pub(crate) fn after_physics_log(
    ticker: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    players: Query<
        (Entity, &Position, &Rotation),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<&Position, (With<BallMarker>, Without<Confirmed>)>,
) {
    let mut tick = ticker.tick();
    if let Some(rollback) = rollback {
        if let RollbackState::ShouldRollback { current_tick } = rollback.state {
            tick = current_tick;
        }
    }
    for (entity, position, rotation) in players.iter() {
        trace!(
            ?tick,
            ?entity,
            ?position,
            ?rotation,
            "Player after physics update"
        );
    }
    for position in ball.iter() {
        debug!(?tick, ?position, "Ball after physics update");
    }
}

pub(crate) fn last_log(
    ticker: Res<TickManager>,
    players: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Option<&Correction<Position>>,
            Option<&Correction<Rotation>>,
        ),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<&Position, (With<BallMarker>, Without<Confirmed>)>,
) {
    let tick = ticker.tick();
    for (entity, position, rotation, correction, rotation_correction) in players.iter() {
        trace!(?tick, ?entity, ?position, ?correction, "Player LAST update");
        trace!(
            ?tick,
            ?entity,
            ?rotation,
            ?rotation_correction,
            "Player LAST update"
        );
    }
    for position in ball.iter() {
        debug!(?tick, ?position, "Ball LAST update");
    }
}

pub(crate) fn log() {
    debug!("run physics schedule!");
}


// Wall
#[derive(Bundle)]
pub(crate) struct WallBundle {
    color: ColorComponent,
    physics: PhysicsBundle,
    wall: PbrBundle,
}


impl WallBundle {
    pub(crate) fn new(start: Vec3, end: Vec3, height: f32, thickness: f32, color: Color) -> Self {
        Self {
            color: ColorComponent(color),
            physics: PhysicsBundle {
                collider: Collider::cuboid(thickness, height, 30.0),
                collider_density: ColliderDensity(1.0),
                rigid_body: RigidBody::Static,
            },
            wall: Default::default()
        }
    }
}
