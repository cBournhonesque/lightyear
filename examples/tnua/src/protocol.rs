use crate::shared::color_from_id;
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy_tnua::TnuaScheme;
use bevy_tnua::builtins::*;
use bevy_tnua::prelude::*;
use bevy_tnua_physics_integration_layer::math::float_consts;
use lightyear::input::bei::prelude::InputAction;
use lightyear::input::config::InputConfig;
use lightyear::prelude::input::bei::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

pub const BALL_SIZE: f32 = 15.0;
pub const PLAYER_SIZE: f32 = 40.0;

#[derive(Bundle)]
pub(crate) struct PhysicsBundle {
    pub(crate) collider: Collider,
    pub(crate) collider_density: ColliderDensity,
    pub(crate) rigid_body: RigidBody,
    pub(crate) restitution: Restitution,
}

impl PhysicsBundle {
    pub(crate) fn ball() -> Self {
        Self {
            collider: Collider::circle(BALL_SIZE),
            collider_density: ColliderDensity(0.05),
            rigid_body: RigidBody::Dynamic,
            restitution: Restitution::new(0.5),
        }
    }

    pub(crate) fn player() -> Self {
        Self {
            collider: Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
            collider_density: ColliderDensity(0.2),
            rigid_body: RigidBody::Dynamic,
            restitution: Restitution::new(0.3),
        }
    }
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

// Messages

#[derive(Event, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Ready;

// Channel

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Channel1;

// the context will be replicated
#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct Player;

#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct Movement;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct Jump;

#[derive(TnuaScheme)]
#[scheme(basis = TnuaBuiltinWalk, serde)]
pub enum DemoControlScheme {
    Jump(TnuaBuiltinJump),
    Crouch(TnuaBuiltinCrouch),
    Dash(TnuaBuiltinDash),
    Knockback(TnuaBuiltinKnockback),
    WallSlide(TnuaBuiltinWallSlide, Entity),
    WallJump(TnuaBuiltinJump),
    Climb(TnuaBuiltinClimb, Entity),
}
impl Default for DemoControlSchemeConfig {
    fn default() -> Self {
        Self {
            basis: TnuaBuiltinWalkConfig {
                float_height: 2.0,
                headroom: Some(TnuaBuiltinWalkHeadroom {
                    distance_to_collider_top: 1.0,
                    ..Default::default()
                }),
                max_slope: float_consts::FRAC_PI_4,
                ..Default::default()
            },
            jump: TnuaBuiltinJumpConfig {
                height: 4.0,
                ..Default::default()
            },
            crouch: TnuaBuiltinCrouchConfig {
                float_offset: -0.9,
                ..Default::default()
            },
            dash: TnuaBuiltinDashConfig {
                horizontal_distance: 10.0,
                vertical_distance: 0.0,
                ..Default::default()
            },
            knockback: Default::default(),
            wall_slide: TnuaBuiltinWallSlideConfig {
                maintain_distance: Some(0.7),
                ..Default::default()
            },
            wall_jump: TnuaBuiltinJumpConfig {
                height: 4.0,
                takeoff_extra_gravity: 90.0, // 3 times the default
                takeoff_above_velocity: 0.0,
                horizontal_distance: 2.0,
                ..Default::default()
            },
            climb: TnuaBuiltinClimbConfig {
                climb_speed: 10.0,
                ..Default::default()
            },
        }
    }
}

// Protocol
#[derive(Clone)]
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        app.add_plugins(InputPlugin::<Player> {
            config: InputConfig {
                rebroadcast_inputs: false,
                ..default()
            },
        });
        app.register_input_action::<Movement>();
        app.register_input_action::<Jump>();

        // channel
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ClientToServer);

        // messages
        app.register_event::<Ready>()
            .add_direction(NetworkDirection::ClientToServer);

        // components
        app.register_component::<PlayerId>();

        // add prediction for non-networked components
        app.register_component::<Position>()
            // register a linear interpolation function without actually running Interpolation systems
            // it will be used for FrameInterpolation
            .register_linear_interpolation()
            .add_linear_correction_fn();

        app.register_component::<Rotation>()
            .register_linear_interpolation()
            .add_linear_correction_fn();

        // NOTE: interpolation/correction is only needed for components that are visually displayed!
        // we still need prediction to be able to correctly predict the physics on the client
        app.register_component::<LinearVelocity>();
        app.register_component::<AngularVelocity>();
    }
}
