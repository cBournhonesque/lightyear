//! Provides a system parameter for performing spatial queries while doing lag compensation.
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::history::{
    BroadPhaseAabbEnvelopeHolder, LagCompensationConfig, LagCompensationHistory, LagCompensationSet,
};
use lightyear::prelude::client::InterpolationDelay;
use lightyear::prelude::TickManager;
#[cfg(all(feature = "2d", not(feature = "3d")))]
use {
    avian2d::{math::*, prelude::*},
    bevy::math::Dir2 as Dir,
};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use {
    avian3d::{math::*, prelude::*},
    bevy::math::Dir3 as Dir,
};

/// A system parameter for performing [spatial queries](spatial_query) while doing
/// lag compensation.
///
/// Systems using this parameter should run after the [`LagCompensationSet::UpdateHistory`] set.
#[derive(SystemParam)]
pub struct LagCompensationSpatialQuery<'w, 's> {
    pub tick_manager: Res<'w, TickManager>,
    pub config: Res<'w, LagCompensationConfig>,
    spatial_query: SpatialQuery<'w, 's>,
    parent_query: Query<'w, 's, (&'static Collider, &'static LagCompensationHistory)>,
    child_query: Query<'w, 's, &'static Parent, With<BroadPhaseAabbEnvelopeHolder>>,
}

impl LagCompensationSpatialQuery<'_, '_> {
    /// Similar to [`SpatialQuery::cast_ray`], but does lag compensation by
    /// using the history buffer of the entity.
    pub fn cast_ray(
        &self,
        interpolation_delay: InterpolationDelay,
        origin: Vector,
        direction: Dir,
        max_distance: Scalar,
        solid: bool,
        filter: &mut SpatialQueryFilter,
    ) -> Option<RayHitData> {
        self.cast_ray_predicate(
            interpolation_delay,
            origin,
            direction,
            max_distance,
            solid,
            &|_| true,
            filter,
        )
    }

    /// Similar to [`SpatialQuery::cast_ray_predicate`], but does lag compensation by
    /// using the history buffer of the entity.
    pub fn cast_ray_predicate(
        &self,
        interpolation_delay: InterpolationDelay,
        origin: Vector,
        direction: Dir,
        max_distance: Scalar,
        solid: bool,
        predicate: &dyn Fn(Entity) -> bool,
        filter: &mut SpatialQueryFilter,
    ) -> Option<RayHitData> {
        // 1): check if the ray hits the aabb envelope
        filter
            .mask
            .add(self.config.broad_phase_envelope_layer_bit as u32);
        let tick = self.tick_manager.tick();
        let tick_duration = self.tick_manager.config.tick_duration;
        let mut exact_hit_data: Option<RayHitData> = None;
        self.spatial_query.cast_ray_predicate(
            origin,
            direction,
            max_distance,
            solid,
            filter,
            &|child| {
                // 2) there is a hit! Check if we hit the collider from the history
                // TODO: use error handling here
                let parent = self
                    .child_query
                    .get(child)
                    .expect("the broad phase entity must have a parent")
                    .get();
                let (collider, history) = self
                    .parent_query
                    .get(parent)
                    .expect("the parent must have a history");
                let (interpolation_tick, interpolation_overstep) =
                    interpolation_delay.tick_and_overstep(tick, tick_duration);

                // find the collider position at that time in history
                // the start corresponds to tick `interpolation_tick` (we interpolate between `interpolation_tick` and `interpolation_tick + 1`)
                let (source_idx, (_, (start_position, start_rotation, _))) = history
                    .into_iter()
                    .enumerate()
                    .find(|(_, (history_tick, _))| *history_tick == interpolation_tick)
                    .unwrap();
                let (_, (target_position, target_rotation, _)) =
                    history.into_iter().skip(source_idx + 1).next().unwrap();
                // we assume that the collider itself doesn't change so we don't need to interpolate it
                let interpolated_position =
                    start_position.lerp(**target_position, interpolation_overstep);
                let interpolated_rotation =
                    start_rotation.slerp(*target_rotation, interpolation_overstep);

                #[cfg(all(feature = "2d", not(feature = "3d")))]
                let dir = direction.as_vec2();
                #[cfg(all(feature = "3d", not(feature = "2d")))]
                let dir = direction.as_vec3();

                if let Some((distance, normal)) = collider.cast_ray(
                    interpolated_position,
                    interpolated_rotation,
                    origin,
                    dir,
                    max_distance,
                    solid,
                ) {
                    if !predicate(parent) {
                        return false;
                    }
                    exact_hit_data = Some(RayHitData {
                        entity: parent,
                        distance,
                        normal,
                    });
                    return true;
                }
                false
            },
        );
        exact_hit_data
    }
}
