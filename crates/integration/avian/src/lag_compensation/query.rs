//! Provides a system parameter for performing spatial queries while doing lag compensation.
use core::cell::RefCell;

use super::history::{AabbEnvelopeHolder, LagCompensationHistory};
use bevy_ecs::prelude::*;
use bevy_ecs::system::SystemParam;
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_interpolation::plugin::InterpolationDelay;
use lightyear_link::prelude::Server;
#[allow(unused_imports)]
use tracing::{debug, error, info};
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

/// Historical collider pose sampled for one lag-compensated query time.
///
/// This is useful for diagnostics even when a spatial query misses: a server
/// can draw the exact collider pose that its narrow phase would have tested.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LagCompensationSample {
    /// The first history tick used to interpolate the collider pose.
    pub interpolation_tick: Tick,
    /// The interpolation fraction between `interpolation_tick` and the next
    /// history tick.
    pub interpolation_overstep: Scalar,
    /// The collider position used by the narrow-phase query.
    pub position: Position,
    /// The collider rotation used by the narrow-phase query.
    pub rotation: Rotation,
}

/// A lag-compensated ray hit together with the historical pose used for the
/// narrow-phase collision test.
///
/// The regular [`RayHitData`] identifies what was hit and where along the ray
/// it was hit. The additional fields make the rewind observable: debug tools
/// can draw the collider at the exact interpolated pose that produced the hit
/// instead of guessing from the current server transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LagCompensationRayHit {
    /// The result of testing the ray against the historical collider.
    pub hit: RayHitData,
    /// The first history tick used to interpolate the collider pose.
    pub interpolation_tick: Tick,
    /// The interpolation fraction between `interpolation_tick` and the next
    /// history tick.
    pub interpolation_overstep: Scalar,
    /// The collider position used by the narrow-phase query.
    pub position: Position,
    /// The collider rotation used by the narrow-phase query.
    pub rotation: Rotation,
}

/// A system parameter for performing [spatial queries](spatial_query) while doing
/// lag compensation.
///
/// Systems using this parameter should run after the [`LagCompensationSet::UpdateHistory`](super::history::LagCompensationSystems) set.
#[derive(SystemParam)]
pub struct LagCompensationSpatialQuery<'w, 's> {
    pub timeline: Res<'w, LocalTimeline>,
    server: Single<'w, 's, (), With<Server>>,
    spatial_query: SpatialQuery<'w, 's>,
    parent_query: Query<
        'w,
        's,
        (
            &'static Collider,
            &'static CollisionLayers,
            &'static LagCompensationHistory,
        ),
    >,
    child_query: Query<'w, 's, &'static ChildOf, With<AabbEnvelopeHolder>>,
}

impl LagCompensationSpatialQuery<'_, '_> {
    /// Sample one collider at the same historical time used by lag-compensated
    /// spatial queries.
    ///
    /// Unlike the `*_with_sample` query methods, this also returns a pose when
    /// no ray or shape intersects the collider. It is primarily useful for
    /// diagnostics that need to explain a miss.
    pub fn sample_collider(
        &self,
        interpolation_delay: InterpolationDelay,
        entity: Entity,
    ) -> Option<LagCompensationSample> {
        let (_, _, history) = self.parent_query.get(entity).ok()?;
        sample_history(history, interpolation_delay, self.timeline.tick(), false)
    }

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
        self.cast_ray_with_sample(
            interpolation_delay,
            origin,
            direction,
            max_distance,
            solid,
            filter,
        )
        .map(|result| result.hit)
    }

    /// Like [`Self::cast_ray`], but also returns the exact historical collider
    /// pose used for the successful narrow-phase test.
    pub fn cast_ray_with_sample(
        &self,
        interpolation_delay: InterpolationDelay,
        origin: Vector,
        direction: Dir,
        max_distance: Scalar,
        solid: bool,
        filter: &mut SpatialQueryFilter,
    ) -> Option<LagCompensationRayHit> {
        self.cast_ray_predicate_with_sample(
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
        self.cast_ray_predicate_with_sample(
            interpolation_delay,
            origin,
            direction,
            max_distance,
            solid,
            predicate,
            filter,
        )
        .map(|result| result.hit)
    }

    /// Like [`Self::cast_ray_predicate`], but also returns the exact historical
    /// collider pose used for the successful narrow-phase test.
    #[allow(clippy::too_many_arguments)]
    pub fn cast_ray_predicate_with_sample(
        &self,
        interpolation_delay: InterpolationDelay,
        origin: Vector,
        direction: Dir,
        max_distance: Scalar,
        solid: bool,
        predicate: &dyn Fn(Entity) -> bool,
        filter: &mut SpatialQueryFilter,
    ) -> Option<LagCompensationRayHit> {
        // 1): check if the ray hits the aabb envelope
        let tick = self.timeline.tick();
        // we use interior mutability because the predicate must be a `dyn Fn`
        let exact_hit_data: RefCell<Option<LagCompensationRayHit>> = RefCell::new(None);
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
                let Some(sample) = sample_history(history, interpolation_delay, tick, true) else {
                    return false;
                };

                #[cfg(all(feature = "2d", not(feature = "3d")))]
                let dir = direction.as_vec2().adjust_precision();
                #[cfg(all(feature = "3d", not(feature = "2d")))]
                let dir = direction.as_vec3().adjust_precision();

                if let Some((distance, normal)) = collider.cast_ray(
                    sample.position.0,
                    sample.rotation,
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
                        interpolation_tick = ?sample.interpolation_tick,
                        interpolation_overstep = ?sample.interpolation_overstep,
                        interpolated_position = ?sample.position,
                        ?parent,
                        "LagCompensation RayHit!"
                    );
                    *exact_hit_data.borrow_mut() = Some(LagCompensationRayHit {
                        hit: RayHitData {
                            entity: parent,
                            distance,
                            normal,
                        },
                        interpolation_tick: sample.interpolation_tick,
                        interpolation_overstep: sample.interpolation_overstep,
                        position: sample.position,
                        rotation: sample.rotation,
                    });
                    return true;
                }
                false
            },
        );
        exact_hit_data.into_inner()
    }
}

fn sample_history(
    history: &LagCompensationHistory,
    interpolation_delay: InterpolationDelay,
    current_tick: Tick,
    log_missing_history: bool,
) -> Option<LagCompensationSample> {
    let (interpolation_tick, interpolation_overstep) =
        interpolation_delay.tick_and_overstep(current_tick);

    // The start corresponds to `interpolation_tick`; the query interpolates
    // toward the immediately following history sample.
    let Some((source_idx, (_, (start_position, start_rotation, _)))) = history
        .into_iter()
        .enumerate()
        .find(|(_, (history_tick, _))| *history_tick == interpolation_tick)
    else {
        let oldest_tick = history.oldest().map(|(tick, _)| *tick);
        let recent_tick = history.most_recent().map(|(tick, _)| *tick);
        if log_missing_history {
            error!(
                ?oldest_tick,
                ?recent_tick,
                ?interpolation_tick,
                "Could not find history tick matching interpolation_tick"
            );
        }
        return None;
    };
    let Some((_, (target_position, target_rotation, _))) = history.into_iter().nth(source_idx + 1)
    else {
        if log_missing_history {
            debug!(
                ?interpolation_tick,
                ?source_idx,
                "Could not find history tick after interpolation_tick"
            );
        }
        return None;
    };
    let interpolation_overstep = Scalar::from(interpolation_overstep);

    // The collider shape is assumed to be stable; only its pose is sampled.
    let position = start_position.lerp(**target_position, interpolation_overstep);
    let rotation = start_rotation.slerp(*target_rotation, interpolation_overstep);
    Some(LagCompensationSample {
        interpolation_tick,
        interpolation_overstep,
        position: Position(position),
        rotation,
    })
}
