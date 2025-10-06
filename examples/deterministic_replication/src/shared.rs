use crate::protocol::*;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use core::hash::{Hash, Hasher};
use leafwing_input_manager::input_map::InputMap;
use leafwing_input_manager::prelude::ActionState;
use lightyear::connection::client_of::ClientOf;
use lightyear::connection::host::HostClient;
use lightyear::input::input_buffer::InputBuffer;
use lightyear::input::leafwing::prelude::LeafwingSnapshot;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::rollback::{DeterministicPredicted, DisableRollback};
use lightyear::prelude::*;
use lightyear_avian2d::plugin::AvianReplicationMode;
use lightyear_frame_interpolation::FrameInterpolate;

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
        app.add_plugins(lightyear_avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position,
            rollback_resources: true,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable syncing position<>transform as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>()
                // interpolation is handled by lightyear_frame_interpolation
                .disable::<PhysicsInterpolationPlugin>()
                // disable island sleeping plugin as it's not compatible with rollbacks
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));

        // Game logic
        app.add_systems(FixedUpdate, player_movement);

        // DEBUG
        // app.add_systems(
        //     RunFixedMainLoop,
        //     debug.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
        // );
        // app.add_systems(
        //     FixedPreUpdate,
        //     fixed_pre_log.after(InputSet::BufferClientInputs),
        // );
        // app.add_systems(FixedPostUpdate, fixed_pre_prepare
        //     .after(PhysicsSet::First)
        //     .before(PhysicsSet::Prepare));
        // app.add_systems(FixedPostUpdate, fixed_pre_physics
        //     .after(PhysicsSet::Prepare)
        //     .before(PhysicsSet::StepSimulation));
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

pub(crate) fn player_bundle(peer_id: PeerId) -> impl Bundle {
    let y = (peer_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    (
        Position::from(Vec2::new(-50.0, y)),
        ColorComponent(Color::BLACK),
        PhysicsBundle::player(),
        Name::from("Player"),
        // this indicates that the entity will only do rollbacks from input updates, and not state updates!
        // It is REQUIRED to add this component to indicate which entities will be rollbacked
        // in deterministic replication mode.
        DeterministicPredicted,
        // this is a bit subtle:
        // when we add DeterministicPredicted to the entity, we enable it for rollbacks. Since we have RollbackMode::Always,
        // we will try to rollback on every input received. We will therefore rollback to before the entity was spawned,
        // which will immediately despawn the entity!
        // This is because we are not creating the entity in a deterministic way. (if we did, we would be re-creating the
        // entity during the rollbacks). As a workaround, we disable rollbacks for this entity for a few ticks.
        DisableRollback,
    )
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

/// In deterministic replication, the client and server simulates all players.
fn player_movement(
    timeline: Single<&LocalTimeline, Without<ClientOf>>,
    mut velocity_query: Query<(
        Entity,
        &PlayerId,
        &Position,
        &mut LinearVelocity,
        &ActionState<PlayerActions>,
    )>,
) {
    let tick = timeline.tick();
    for (entity, player_id, position, velocity, action_state) in velocity_query.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            trace!(?entity, ?tick, ?position, actions = ?action_state.get_pressed(), "applying movement to predicted player");
            // note that we also apply the input to the other predicted clients! even though
            //  their inputs are only replicated with a delay!
            // TODO: add input decay?
            shared_movement_behaviour(velocity, action_state);
        }
    }
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

pub(crate) fn fixed_pre_prepare(
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
            "Client in FixedPostUpdate right before prepare"
        );
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
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    players: Query<
        (
            Entity,
            &Position,
            Option<&FrameInterpolate<Position>>,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&InputBuffer<LeafwingSnapshot<PlayerActions>>>,
        ),
        (Without<BallMarker>, With<PlayerId>),
    >,
    ball: Query<(&Position, Option<&VisualCorrection<Position>>), With<BallMarker>>,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();
    for (entity, position, interpolate, correction, action_state, input_buffer) in players.iter() {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        info!(
            ?rollback,
            ?tick,
            ?entity,
            ?position,
            ?interpolate,
            ?correction,
            ?pressed,
            ?last_buffer_tick,
            "Player in FixedLast"
        );
    }
    // for (position, correction) in ball.iter() {
    //     info!(?tick, ?position, ?correction, "Ball after physics update");
    // }
}

pub(crate) fn last_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Without<ClientOf>>,
    players: Query<
        (
            Entity,
            &Position,
            &Transform,
            Option<&FrameInterpolate<Position>>,
            Option<&VisualCorrection<Position>>,
            Option<&ActionState<PlayerActions>>,
            Option<&InputBuffer<LeafwingSnapshot<PlayerActions>>>,
        ),
        (Without<BallMarker>, With<PlayerId>),
    >,
    ball: Query<(&Position, Option<&VisualCorrection<Position>>), With<BallMarker>>,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();

    for (entity, position, transform, interpolate, correction, action_state, input_buffer) in
        players.iter()
    {
        let pressed = action_state.map(|a| a.get_pressed());
        let last_buffer_tick = input_buffer.and_then(|b| b.get_last_with_tick().map(|(t, _)| t));
        let translation = transform.translation.truncate();
        info!(
            ?rollback,
            ?tick,
            ?entity,
            ?position,
            ?translation,
            ?interpolate,
            ?correction,
            ?pressed,
            ?last_buffer_tick,
            "Player in Last"
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
