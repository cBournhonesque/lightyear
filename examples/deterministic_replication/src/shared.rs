use crate::protocol::*;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use core::hash::{Hash, Hasher};
use leafwing_input_manager::input_map::InputMap;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client_of::ClientOf;
use lightyear::input::input_buffer::InputBuffer;
use lightyear::prediction::rollback::{DeterministicPredicted, DisableRollback};
use lightyear::prelude::*;

pub(crate) const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;

/// SharedPlugin between the client and server.
///
/// We can choose to make the server a pure relay server (with 0 simulation), or to make it simulate some elements.
#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        // bundles
        app.add_systems(Startup, init);

        // physics
        app.add_plugins(lightyear_avian2d::prelude::LightyearAvianPlugin {
            rollback_resources: true,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable Sync as it is handled by lightyear_avian
                .disable::<SyncPlugin>()
                // interpolation is handled by lightyear_frame_interpolation
                .disable::<PhysicsInterpolationPlugin>()
                // disable Sleeping plugin as it can mess up physics rollbacks
                .disable::<SleepingPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));

        // registry types for reflection
        app.register_type::<PlayerId>();

        // DEBUG
        // app.add_systems(
        //     RunFixedMainLoop,
        //     debug.in_set(RunFixedMainLoopSystem::BeforeFixedMainLoop),
        // );
        // app.add_systems(
        //     FixedPreUpdate,
        //     fixed_pre_log.after(InputSet::BufferClientInputs),
        // );
        // app.add_systems(FixedPostUpdate, fixed_pre_physics.before(PhysicsSet::StepSimulation));
        app.add_systems(FixedLast, fixed_last_log);
        // app.add_systems(Last, last_log);
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn((
        Position::default(),
        ColorComponent(css::AZURE.into()),
        PhysicsBundle::ball(),
        BallMarker,
        DeterministicPredicted,
        DisableRollback,
        Name::from("Ball"),
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(
    mut velocity: Mut<LinearVelocity>,
    action: &ActionState<PlayerActions>,
) {
    trace!(pressed = ?action.get_pressed(), "shared movement");
    const MOVE_SPEED: f32 = 10.0;
    if action.pressed(&PlayerActions::Up) {
        velocity.y += MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Down) {
        velocity.y -= MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Left) {
        velocity.x -= MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Right) {
        velocity.x += MOVE_SPEED;
    }
    *velocity = LinearVelocity(velocity.clamp_length_max(MAX_VELOCITY));
}

fn debug() {
    trace!("Fixed Start");
}

pub(crate) fn fixed_pre_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), With<Client>>,
    remote_client_inputs: Query<
        (
            Entity,
            &ActionState<PlayerActions>,
            &InputBuffer<ActionState<PlayerActions>>,
        ),
        (Without<InputMap<PlayerActions>>, With<Predicted>),
    >,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();
    for (entity, action_state, buffer) in remote_client_inputs.iter() {
        let pressed = action_state.get_pressed();
        info!(
            ?rollback,
            ?tick,
            ?entity,
            ?pressed,
            %buffer,
            "Remote client input before FixedUpdate");
    }
}

pub(crate) fn fixed_pre_physics(
    timeline: Single<(&LocalTimeline, Has<Rollback>), With<Client>>,
    remote_client_inputs: Query<
        (
            Entity,
            &Position,
            &LinearVelocity,
            &ActionState<PlayerActions>,
        ),
        With<Predicted>,
    >,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();
    for (entity, position, velocity, action_state) in remote_client_inputs.iter() {
        let pressed = action_state.get_pressed();
        info!(
            ?rollback,
            ?tick,
            ?entity,
            ?position,
            ?velocity,
            ?pressed,
            "Client in FixedPostUpdate right before physics"
        );
    }
}

pub(crate) fn fixed_last_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Or<(With<Client>, Without<ClientOf>)>>,
    players: Query<
        (
            Entity,
            &Position,
            &LinearVelocity,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&InputBuffer<ActionState<PlayerActions>>>,
        ),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<
        (&Position, Option<&VisualCorrection<Position>>),
        (With<BallMarker>, Without<Confirmed>),
    >,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();

    for (entity, position, velocity, correction, action_state, input_buffer) in players.iter() {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        info!(
            ?rollback,
            ?tick,
            ?entity,
            ?position,
            ?velocity,
            ?correction,
            ?pressed,
            ?last_buffer_tick,
            "Player after physics update"
        );
    }
    // for (position, correction) in ball.iter() {
    //     info!(?tick, ?position, ?correction, "Ball after physics update");
    // }
}

pub(crate) fn last_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Or<(With<Client>, Without<ClientOf>)>>,
    players: Query<
        (
            Entity,
            &Position,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&InputBuffer<ActionState<PlayerActions>>>,
        ),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<
        (&Position, Option<&VisualCorrection<Position>>),
        (With<BallMarker>, Without<Confirmed>),
    >,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();

    for (entity, position, correction, action_state, input_buffer) in players.iter() {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        trace!(
            ?rollback,
            ?tick,
            ?entity,
            ?position,
            ?correction,
            ?pressed,
            ?last_buffer_tick,
            "Player after physics update"
        );
    }
    // for (position, correction) in ball.iter() {
    //     info!(?tick, ?position, ?correction, "Ball after physics update");
    // }
}

// Wall
#[derive(Bundle)]
pub(crate) struct WallBundle {
    color: ColorComponent,
    physics: PhysicsBundle,
    wall: Wall,
    name: Name,
}

#[derive(Component)]
pub(crate) struct Wall {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
}

impl WallBundle {
    pub(crate) fn new(start: Vec2, end: Vec2, color: Color) -> Self {
        Self {
            color: ColorComponent(color),
            physics: PhysicsBundle {
                collider: Collider::segment(start, end),
                collider_density: ColliderDensity(1.0),
                rigid_body: RigidBody::Static,
                restitution: Restitution::new(0.0),
            },
            wall: Wall { start, end },
            name: Name::from("Wall"),
        }
    }
}
