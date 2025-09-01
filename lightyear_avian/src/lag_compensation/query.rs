//! Provides a system parameter for performing spatial queries while doing lag compensation.
use core::cell::RefCell;

use super::history::{AabbEnvelopeHolder, LagCompensationHistory};
use bevy_ecs::{
    entity::Entity,
    hierarchy::ChildOf,
    query::With,
    system::{Query, SystemParam},
};
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_interpolation::plugin::InterpolationDelay;
use lightyear_link::prelude::Server;
#[allow(unused_imports)]
use tracing::{debug, info, error};
#[cfg(all(feature = "2d", not(feature = "3d")))]
use {
    avian2d::{math::*, prelude::*},
    bevy_math::Dir2 as Dir,
};
#[cfg(all(feature = "3d", not(feature = "2d")))]
use {
    avian3d::{math::*, prelude::*},
    bevy_math::Dir3 as Dir,
};

/// A system parameter for performing [spatial queries](spatial_query) while doing
/// lag compensation.
///
/// Systems using this parameter should run after the [`LagCompensationSet::UpdateHistory`](super::history::LagCompensationSet) set.
#[derive(SystemParam)]
pub struct LagCompensationSpatialQuery<'w, 's> {
    pub timeline: Query<'w, 's, &'static LocalTimeline, With<Server>>,
    spatial_query: SpatialQuery<'w, 's>,
    parent_query: Query<'w, 's, (&'static Collider, &'static CollisionLayers, &'static LagCompensationHistory)>,
    child_query: Query<'w, 's, &'static ChildOf, With<AabbEnvelopeHolder>>,
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
    #[allow(clippy::too_many_arguments)]
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
        let timeline = self.timeline.single().ok()?;
        let tick = timeline.tick();
        // we use interior mutability because the predicate must be a `dyn Fn`
        let exact_hit_data: RefCell<Option<RayHitData>> = RefCell::new(None);
        self.spatial_query.cast_ray_predicate(
            origin,
            direction,
            max_distance,
            solid,
            // TODO: the user could have excluded the Parent entity from the filter, which would do nothing
            //  since we are checking collisions with the child!
            filter,
            &|child| {
                // 2) there is a hit! Check if we hit the collider from the history

                // we cannot rely directly on the CollisionLayers to filter contacts with aabb envelopes
                // because CollisionLayers only encodes OR conditions, not AND
                let Ok(parent_component) = self.child_query.get(child) else {
                    return false;
                };
                let parent = parent_component.parent();
                debug!(?parent, ?filter, "Broadphase hit with {child:?}");
                let (collider, collision_layers, history) = self
                    .parent_query
                    .get(parent)
                    .expect("the parent must have a history");
                // the collisions are done with the lag compensation collider; make sure that the parent is not excluded
                if !filter.test(parent, *collision_layers) {
                    debug!("Collider entity {parent:?} with layers {collision_layers:?} excluded because of filter");
                    return false;
                }
                let (interpolation_tick, interpolation_overstep) =
                    interpolation_delay.tick_and_overstep(tick);

                // find the collider position at that time in history
                // the start corresponds to tick `interpolation_tick` (we interpolate between `interpolation_tick` and `interpolation_tick + 1`)
                let Some((source_idx, (_, (start_position, start_rotation, _)))) = history
                    .into_iter()
                    .enumerate()
                    .find(|(_, (history_tick, _))| *history_tick == interpolation_tick)
                else {
                    let oldest_tick = history.oldest().map(|(tick, _)| *tick);
                    let recent_tick = history.most_recent().map(|(tick, _)| *tick);
                    error!(
                        ?oldest_tick,
                        ?recent_tick,
                        ?interpolation_tick,
                        "Could not find history tick matching interpolation_tick"
                    );
                    return false;
                };
                // TODO: handle this in host-server mode!
                let (_, (target_position, target_rotation, _)) =
                    history.into_iter().nth(source_idx + 1).unwrap();
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
                    debug!(
                        ?tick,
                        ?interpolation_tick,
                        ?interpolation_overstep,
                        ?interpolated_position,
                        ?parent,
                        "LagCompensation RayHit!"
                    );
                    *exact_hit_data.borrow_mut() = Some(RayHitData {
                        entity: parent,
                        distance,
                        normal,
                    });
                    return true;
                }
                false
            },
        );
        exact_hit_data.into_inner()
    }
}
