//! Network protocol shared by the projectiles client and server.
//!
//! The four axis values are separate replicated components on the global
//! `ClientContext` entity. Keeping them independent is the key structural
//! change in this refactor: timeline selection no longer silently chooses hit
//! authority, and trajectory no longer chooses network representation.

use avian2d::prelude::*;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear::input::prelude::InputConfig;
use lightyear::prelude::input::bei::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::hit_detection::HitPolicy;
use crate::representation::{
    RepresentationKind,
    fire_data_entity::FireData,
    shot_buffer::{ShotBuffer, interpolate_shot_buffer},
};
use crate::shared::{DespawnAtTick, ProjectileFireTick};
use crate::timeline::TimelinePolicy;
use crate::trajectory::{TrajectoryKind, hitscan::HitscanVisual};

pub const BULLET_SIZE: f32 = 3.0;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Bot;

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerMarker;

/// Number of authoritative hits credited to this player.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Score(pub usize);

#[derive(Component, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct ColorComponent(pub(crate) Color);

/// Identifies the player who fired a projectile.
///
/// Use `PeerId`, not `Entity`, so this component needs no cross-world entity
/// mapping and remains meaningful in logs from different processes.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct BulletMarker {
    pub shooter: PeerId,
}

// Player input context and actions.
#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct PlayerContext;

#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct MovePlayer;

#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct MoveCursor;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct Shoot;

// Global example configuration context and one action for each axis.
#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct ClientContext;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct CycleTrajectory;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct CycleRepresentation;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct CycleHitPolicy;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct CycleTimeline;

/// Per-player cadence state. Projectile history does not belong here; the
/// shot-buffer representation keeps its sequenced ring in a separate component.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Reflect)]
pub struct Weapon {
    pub last_fire_tick: Option<Tick>,
}

/// Intentionally insecure client-to-server hit claim used by the
/// `ClientReported` policy.
#[derive(MapEntities, Event, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HitDetected {
    #[entities]
    pub shooter: Entity,
    #[entities]
    pub target: Entity,
}

pub struct HitChannel;

pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(InputPlugin::new(InputConfig::<PlayerContext> {
            // Input messages carry interpolation delay for the rewound-server
            // policy. The server toggles rebroadcasting when the timeline axis
            // changes to/from AllPredicted.
            lag_compensation: true,
            rebroadcast_inputs: true,
            ..default()
        }));
        app.register_input_action::<MovePlayer>();
        app.register_input_action::<MoveCursor>();
        app.register_input_action::<Shoot>();

        app.add_plugins(InputPlugin::new(InputConfig::<ClientContext> {
            // Axis-selection actions are UI commands, not predicted gameplay.
            ignore_rollbacks: true,
            ..default()
        }));
        app.register_input_action::<CycleTrajectory>();
        app.register_input_action::<CycleRepresentation>();
        app.register_input_action::<CycleHitPolicy>();
        app.register_input_action::<CycleTimeline>();

        app.add_channel::<HitChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);

        app.register_event::<HitDetected>()
            .add_map_entities()
            .add_direction(NetworkDirection::ClientToServer);

        // Registration order is shared by client and server. Keep it stable.
        app.component::<Name>().replicate();
        app.component::<PlayerId>().replicate();
        app.component::<PlayerMarker>().replicate();
        app.component::<ColorComponent>().replicate();
        app.component::<Score>().replicate();
        app.component::<Bot>().replicate();

        app.component::<BulletMarker>().replicate();
        app.component::<ProjectileFireTick>().replicate().predict();
        // Start/end are predicted state, while lifetime is local presentation.
        // Ignore lifetime drift when deciding whether authoritative geometry
        // requires a rollback.
        app.component::<HitscanVisual>()
            .replicate()
            .predict()
            .with_rollback_condition(hitscan_geometry_should_rollback);
        // The fire-data parent itself is the owner's prespawned predicted
        // entity. Tracking its immutable firing facts lets authoritative data
        // participate in rollback instead of merely confirming entity identity.
        app.component::<FireData>().replicate().predict();
        app.component::<RigidBody>().replicate();

        app.component::<Weapon>().replicate().predict();
        app.component::<ShotBuffer>()
            .replicate_diff()
            .predict_diff()
            .add_interpolation_diff_with(interpolate_shot_buffer);

        app.component::<TrajectoryKind>().replicate();
        app.component::<RepresentationKind>().replicate();
        app.component::<HitPolicy>().replicate();
        app.component::<TimelinePolicy>().replicate();

        // Predicted projectiles derive the same fixed expiry from their fire
        // tick. Restore it together with the rest of local projectile state.
        app.local_rollback::<DespawnAtTick>();
    }
}

fn hitscan_geometry_should_rollback(confirmed: &HitscanVisual, predicted: &HitscanVisual) -> bool {
    const GEOMETRY_EPSILON_SQUARED: f32 = 0.0001 * 0.0001;
    confirmed.start.distance_squared(predicted.start) > GEOMETRY_EPSILON_SQUARED
        || confirmed.end.distance_squared(predicted.end) > GEOMETRY_EPSILON_SQUARED
}
