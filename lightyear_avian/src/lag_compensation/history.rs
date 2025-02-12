/// This plugin maintains a history buffer of the Position, Rotation and ColliderAabb of server entities
/// so that they can be used for lag compensation.
use bevy::prelude::*;
use lightyear::prelude::{HistoryBuffer, TickManager};

#[cfg(all(feature = "2d", not(feature = "3d")))]
use avian2d::{math::Vector, prelude::*};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use avian3d::{math::Vector, prelude::*};

/// Add this plugin to enable lag compensation on the server
#[derive(Resource)]
pub struct LagCompensationPlugin;

/// This resource contains some configuration options for lag compensation
#[derive(Resource)]
pub struct LagCompensationConfig {
    /// Maximum number of ticks that we will store in the history buffer for lag compensation.
    /// This will determine how far back in time we can rewind the entity's position.
    ///
    /// Around 300ms should be enough for most cases
    pub max_collider_history_ticks: u8,
}

impl Default for LagCompensationConfig {
    fn default() -> Self {
        Self {
            // 33 ticks corresponds to ~500ms assuming 64Hz ticks
            max_collider_history_ticks: 35,
        }
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum LagCompensationSet {
    /// Update the broad phase collider history
    ///
    /// Any t needs to perform some lag-compensation query using the history
    /// should run after this set
    UpdateHistory,
    /// Compute collisions using lag compensation
    Collisions,
}

/// Marker component to indicate that this collider's [ColliderAabb] holds the
/// broad-phase AABB envelope of its parent (the entity for which we want to apply
/// lag compensation)
#[derive(Component)]
pub struct AabbEnvelopeHolder;

/// Component that will store the Position, Rotation, ColliderAabb in a history buffer
/// in order to perform lag compensation for client-predicted entities interacting with
/// this entity
pub type LagCompensationHistory = HistoryBuffer<(Position, Rotation, ColliderAabb)>;

impl Plugin for LagCompensationPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LagCompensationHistory>();

        app.init_resource::<LagCompensationConfig>();
        app.add_observer(spawn_broad_phase_aabb_envelope);
        // We want the history buffer at tick N to contain the collider state (Position, Rotation)
        // AFTER the PhysicsSet::Step has run. (one way to reason about this is that the server
        // sends the collider state at tick N in post-update, also after the physics simulation step has run)
        //
        // The ColliderAABB gets updated in the BroadPhase set (before the Solver step) which might cause
        // a 1-tick delay but that shouldn't matter much because we are just using it to compute an aabb envelope
        // of all ticks
        app.add_systems(
            PhysicsSchedule,
            (update_collision_layers, update_collider_history)
                .in_set(LagCompensationSet::UpdateHistory),
        );

        app.configure_sets(
            PhysicsSchedule,
            (
                PhysicsStepSet::Solver,
                // the history must be updated before the SpatialQuery is updated
                LagCompensationSet::UpdateHistory.ambiguous_with(PhysicsStepSet::ReportContacts),
                PhysicsStepSet::SpatialQuery,
                // collisions must run after the SpatialQuery has been updated
                LagCompensationSet::Collisions,
            )
                .chain(),
        );
        app.configure_sets(
            FixedPostUpdate,
            LagCompensationSet::Collisions.after(PhysicsSet::Sync),
        );
    }
}

/// Spawns a child entity with a collider that represents the broad-phase aabb envelope
/// for lag compensation purposes
fn spawn_broad_phase_aabb_envelope(
    trigger: Trigger<OnAdd, LagCompensationHistory>,
    query: Query<Option<&CollisionLayers>>,
    mut commands: Commands,
) {
    debug!("spawning broad-phase collider from aabb!");
    commands.entity(trigger.target()).with_children(|builder| {
        let mut child_commands = builder.spawn((
            // the collider/position/rotation values don't matter here because they will be updated in the
            // `update_lag_compensation_broad_phase_collider` system
            #[cfg(all(feature = "2d", not(feature = "3d")))]
            Collider::rectangle(1.0, 1.0),
            #[cfg(all(feature = "3d", not(feature = "2d")))]
            Collider::cuboid(1.0, 1.0, 1.0),
            Position::default(),
            Rotation::default(),
            AabbEnvelopeHolder,
        ));
        // the aabb_envelope has the same collision_layers as the parent
        if let Ok(Some(collision_layers)) = query.get(trigger.target()) {
            child_commands.insert(collision_layers.clone());
        }
    });
}

/// Update the collision layers of the child AabbEnvelopeHolder to match the parent
fn update_collision_layers(
    mut child_query: Query<&mut CollisionLayers, With<AabbEnvelopeHolder>>,
    mut parent_query: Query<(&mut CollisionLayers, &Children), Without<AabbEnvelopeHolder>>,
) {
    parent_query.iter_mut().for_each(|(layers, children)| {
        if layers.is_changed() || !layers.is_added() {
            for child in children.iter() {
                if let Ok(mut child_layers) = child_query.get_mut(*child) {
                    *child_layers = *layers;
                }
            }
        }
    });
}

/// For each lag-compensated collider, store every tick a copy of the
/// Position, Rotation and ColliderAabb in the history buffer
///
/// The ColliderAabb is used to compute the broad-phase aabb envelope in the broad-phase.
/// The Position and Rotation will be used to compute an interpolated collider in the narrow-phase.
fn update_collider_history(
    tick_manager: Res<TickManager>,
    config: Res<LagCompensationConfig>,
    mut parent_query: Query<
        (
            &Position,
            &Rotation,
            &ColliderAabb,
            &mut LagCompensationHistory,
        ),
        Without<AabbEnvelopeHolder>,
    >,
    mut children_query: Query<(&Parent, &mut Collider, &mut Position), With<AabbEnvelopeHolder>>,
) {
    let tick = tick_manager.tick();
    children_query
        .iter_mut()
        .for_each(|(parent, mut collider, mut position)| {
            let (parent_position, parent_rotation, parent_aabb, mut history) =
                parent_query.get_mut(parent.get()).unwrap();

            // step 1. update the history buffer of the parent
            history.add_update(
                tick,
                (
                    parent_position.clone(),
                    parent_rotation.clone(),
                    parent_aabb.clone(),
                ),
            );
            history.clear_until_tick(tick - (config.max_collider_history_ticks as u16));

            // step 2. update the child's Position, Rotation, Collider so that the avian spatial query
            //  can use the collider's aabb envelope for broad-phase collision detection
            let (min, max) = history.into_iter().fold(
                (Vector::MAX, Vector::MIN),
                |(min, max), (_, (_, _, aabb))| (min.min(aabb.min), max.max(aabb.max)),
            );
            let aabb_envelope = ColliderAabb::from_min_max(min, max);
            // we cannot use the aabb_envelope directly because the SpatialQuery uses Position, Rotation, Collider
            // instead we will use a cuboid collider with the same dimensions as the aabb envelope, and whose position
            // is the center of the aabb envelope.
            // We don't need to change the Rotation since the aabb envelope is axis-aligned
            #[cfg(all(feature = "2d", not(feature = "3d")))]
            let new_collider = Collider::rectangle(max.x - min.x, max.y - min.y);
            #[cfg(all(feature = "3d", not(feature = "2d")))]
            let new_collider = Collider::cuboid(max.x - min.x, max.y - min.y, max.z - min.z);
            *collider = new_collider;
            *position = Position(aabb_envelope.center());
            trace!(
                ?tick,
                ?history,
                ?aabb_envelope,
                "update collider history and aabb envlope"
            );
        });
}
