use crate::protocol::*;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use bevy_enhanced_input::action::Action;
use bevy_enhanced_input::prelude::{ActionOf, Actions};
use bevy_tnua::builtins::TnuaBuiltinWalk;
use bevy_tnua::{TnuaConfig, TnuaController, TnuaControllerPlugin};
use bevy_tnua_avian2d::TnuaAvian2dPlugin;
use bevy_tnua_physics_integration_layer::math::Vector3;
use core::hash::{Hash, Hasher};
use lightyear::connection::client_of::ClientOf;
use lightyear::connection::host::HostClient;
use lightyear::input::input_buffer::InputBuffer;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::rollback::{DeterministicPredicted, DisableRollback};
use lightyear::prelude::*;
use lightyear_avian2d::plugin::AvianReplicationMode;
use lightyear_frame_interpolation::FrameInterpolate;
use std::ops::Deref;

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

        app.add_plugins((
            TnuaControllerPlugin::<DemoControlScheme>::new(FixedUpdate),
            TnuaAvian2dPlugin::new(FixedUpdate),
        ));

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
        // app.add_systems(Last, last_log);
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn((
        Position::default(),
        ColorComponent(css::AZURE.into()),
        PhysicsBundle::ball(),
        BallMarker,
        DeterministicPredicted {
            skip_despawn: true,
            ..default()
        },
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

pub(crate) fn player_bundle(
    peer_id: PeerId,
    control_scheme_config_assets: &mut Assets<DemoControlSchemeConfig>,
) -> impl Bundle {
    let color = color_from_id(peer_id);
    let y = (peer_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    (
        Position::from(Vec2::new(-50.0, y)),
        ColorComponent(color),
        PhysicsBundle::player(),
        Name::from("Player"),
        add_tnua_components(control_scheme_config_assets),
    )
}

pub(crate) fn add_tnua_components(
    control_scheme_config_assets: &mut Assets<DemoControlSchemeConfig>,
) -> impl Bundle {
    (
        TnuaController::<DemoControlScheme>::default(),
        TnuaConfig::<DemoControlScheme>(
            control_scheme_config_assets.add(DemoControlSchemeConfig::default()),
        ),
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
    mut controller: Mut<TnuaController<DemoControlScheme>>,
    jumps: bool,
    moves: Vec2,
) {
    controller.initiate_action_feeding();
    let up_direction = controller.up_direction().unwrap_or(Dir3::Y);

    let is_climbing =
        controller.action_discriminant() == Some(DemoControlSchemeActionDiscriminant::Climb);

    // Set the basis every frame. Even if the player doesn't move - just use `desired_velocity:
    // Vec3::ZERO` to reset the previous frame's input.
    controller.basis = TnuaBuiltinWalk {
        // The `desired_motion` determines how the character will move.
        desired_motion: moves.extend(0.0).normalize_or_zero(),
        // The other field is `desired_forward` - but since the character model is a capsule we
        // don't care the direction its "forward" is pointing.
        ..Default::default()
    };
    // Feed the jump action every frame as long as the player holds the jump button. If the player
    // stops holding the jump button, simply stop feeding the action.
    if jumps {
        controller.action(DemoControlScheme::Jump(Default::default()));
    }
}

/// In deterministic replication, the client and server simulates all players.
fn player_movement(
    timeline: Res<LocalTimeline>,
    actions: Query<(&Action<Jump>, &Action<Movement>, &ActionOf<Player>)>,
    mut controller_query: Query<&mut TnuaController<DemoControlScheme>, Without<Interpolated>>,
) {
    for (jumps, moves, action_of) in actions.iter() {
        let jumps = *jumps.deref();
        let moves = *moves.deref();
        if let Ok(controller) = controller_query.get_mut(*action_of.deref()) {
            shared_movement_behaviour(controller, jumps, moves);
        }
    }
}

fn debug() {
    trace!("Fixed Start");
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
