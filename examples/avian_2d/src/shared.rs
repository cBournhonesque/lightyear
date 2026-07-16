use crate::protocol::*;
use avian2d::prelude::*;
use bevy::prelude::*;
use core::hash::{Hash, Hasher};
use leafwing_input_manager::prelude::ActionState;
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear::prelude::*;

pub(crate) const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;

/// Local-only marker for the deterministic child collider in the `Player` template.
#[derive(Component)]
pub(crate) struct PlayerChildCollider;

impl PlayerChildCollider {
    pub(crate) fn local_transform() -> Transform {
        Transform::from_translation(CHILD_CUBE_OFFSET.extend(0.0))
    }

    pub(crate) fn collider() -> Collider {
        Collider::rectangle(CHILD_CUBE_SIZE, CHILD_CUBE_SIZE)
    }
}

/// Spawn the fixed-offset collider that is part of a newly added player.
///
/// ```text
/// player cube (rigid body + collider)
/// └── smaller cube collider (no rigid body, fixed local offset)
/// ```
fn spawn_player_child_collider(trigger: On<Add, PlayerId>, mut commands: Commands) {
    let player = trigger.entity;
    commands.spawn((
        ChildOf(player),
        PlayerChildCollider,
        PlayerChildCollider::local_transform(),
        PlayerChildCollider::collider(),
        ColliderOf { body: player },
        ColliderDensity(0.1),
        Restitution::new(0.3),
        CollisionLayers::default(),
        Name::from("PlayerOffsetCubeCollider"),
    ));
}

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_observer(spawn_player_child_collider);
        // bundles
        app.add_systems(Startup, init);

        // physics
