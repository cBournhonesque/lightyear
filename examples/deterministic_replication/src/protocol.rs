use avian2d::prelude::*;
use avian2d::{
    collision::contact_types::ContactGraph,
    dynamics::solver::{constraint_graph::ConstraintGraph, islands::PhysicsIslands},
};
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::input::config::InputConfig;
use lightyear::prelude::input::leafwing;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{AppCatchUpExt, CatchUpGated};
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
            restitution: Restitution::new(1.0),
        }
    }

    pub(crate) fn player() -> Self {
        Self {
            collider: Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
            collider_density: ColliderDensity(0.2),
            rigid_body: RigidBody::Dynamic,
            restitution: Restitution::new(1.0),
        }
    }
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct PlayerActivationTick(pub Tick);

impl PlayerActivationTick {
    pub const DELAY_TICKS: u32 = 30;

    pub fn pending() -> Self {
        Self(Tick(u32::MAX))
    }

    pub fn is_pending(&self) -> bool {
        self.0 == Tick(u32::MAX)
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

// Inputs
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum PlayerActions {
    Up,
    Down,
    Left,
    Right,
}

// Protocol
#[derive(Clone)]
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        app.add_plugins(leafwing::InputPlugin::<PlayerActions> {
            config: InputConfig {
                rebroadcast_inputs: true,
                ..default()
            },
        });

        // Late-join catch-up: shared between client and server so it must
        // be registered before `cli.spawn_connections` adds the Client /
        // ClientOf entities (otherwise `register_required_components`
        // would fail because the archetype already exists). Registered in
        // ProtocolPlugin (loaded by SharedPlugin) so it runs before the
        // CLI spawns the networking entities.
        app.add_plugins(lightyear_deterministic_replication::prelude::LateJoinCatchUpPlugin);
        app.register_catchup_filter::<
            (Position, Rotation, LinearVelocity, AngularVelocity),
            leafwing::LeafwingSequence<PlayerActions>,
        >();
        register_avian_catchup_resources(app, false);

        // components
        app.component::<PlayerId>().replicate();
        app.component::<PlayerActivationTick>().replicate();
        // Physics components are replicated once (initial value on entity spawn)
        // so that late-joining clients get the correct starting state.
        // local_rollback registers PredictionHistory for rollback/checksums.
        // add_confirmed_write ensures the replicated value goes to
        // PredictionHistory as confirmed state (instead of overwriting the
        // component), so input-triggered rollbacks snap to the correct value.
        app.component::<Position>().replicate_once();
        app.local_rollback::<Position>()
            .add_confirmed_write()
            .into_component_registration()
            .add_custom_hash(lightyear_avian2d::types::position::hash)
            .register_linear_interpolation()
            .add_linear_correction_fn();

        app.component::<Rotation>().replicate_once();
        app.local_rollback::<Rotation>()
            .add_confirmed_write()
            .into_component_registration()
            .add_custom_hash(lightyear_avian2d::types::rotation::hash)
            .register_linear_interpolation()
            .add_linear_correction_fn();

        app.component::<LinearVelocity>().replicate_once();
        app.local_rollback::<LinearVelocity>().add_confirmed_write();

        app.component::<AngularVelocity>().replicate_once();
        app.local_rollback::<AngularVelocity>()
            .add_confirmed_write();
    }
}

pub(crate) fn register_avian_catchup_resources(app: &mut App, enable_islands: bool) {
    app.register_catchup::<ContactGraph, leafwing::LeafwingSequence<PlayerActions>>();
    app.register_catchup::<ConstraintGraph, leafwing::LeafwingSequence<PlayerActions>>();
    app.register_required_components::<ContactGraph, CatchUpGated>();
    app.register_required_components::<ConstraintGraph, CatchUpGated>();
    if enable_islands {
        app.register_catchup::<PhysicsIslands, leafwing::LeafwingSequence<PlayerActions>>();
        app.register_required_components::<PhysicsIslands, CatchUpGated>();
    }
}
