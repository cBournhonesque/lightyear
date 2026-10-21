//! Implement lightyear traits for some common bevy types
#[cfg(test)]
use avian2d::math::ToF32Precision;
use avian2d::math::{Real, ToRealPrecision};
use avian2d::prelude::*;
use bevy_math::Vec3;
use bevy_transform::components::Transform;
use bevy_transform_interpolation::hermite::{hermite_quat, hermite_vec3};
use lightyear_interpolation::prelude::InterpolationSampleContext;
use tracing::trace;

#[cfg(feature = "deterministic")]
use core::hash::Hasher;

pub mod position {
    use super::*;

    pub fn lerp(start: &Position, other: &Position, t: f32) -> Position {
        let u = Real::from(t);
        let res = Position::new(start.0 * (1.0 - u) + other.0 * u);
        trace!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }

    #[cfg(feature = "deterministic")]
    pub fn hash(pos: &Position, hasher: &mut seahash::SeaHasher) {
        hasher.write(&pos.x.to_le_bytes());
        hasher.write(&pos.y.to_le_bytes());
    }
}

pub mod rotation {
    use super::*;

    pub fn lerp(start: &Rotation, other: &Rotation, t: f32) -> Rotation {
        let shortest_angle =
            ((((other.as_degrees() - start.as_degrees()) % 360.0) + 540.0) % 360.0) - 180.0;
        let res = Rotation::degrees(start.as_degrees() + shortest_angle * t);
        // // as_radians() returns a value between -Pi and Pi
        // // add Pi to get positive values, for interpolation
        // let res = Rotation::from_radians(
        //     (start.as_radians() + std::f32::consts::PI) * (1.0 - t)
        //         + (other.as_radians() + std::f32::consts::PI) * t,
        // );
        trace!(
            "rotation lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start.as_degrees(),
            other.as_degrees(),
            t,
            res.as_degrees()
        );
        res
    }

    #[cfg(feature = "deterministic")]
    pub fn hash(rot: &Rotation, hasher: &mut seahash::SeaHasher) {
        hasher.write_u32(rot.cos.to_bits());
        hasher.write_u32(rot.sin.to_bits());
    }
}

pub mod linear_velocity {
    use super::*;

    pub fn lerp(start: &LinearVelocity, other: &LinearVelocity, t: f32) -> LinearVelocity {
        let res = LinearVelocity(start.0 * (1.0 - t) + other.0 * t);
        trace!(
            "linear velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }
}

pub mod angular_velocity {
    use super::*;

    pub fn lerp(start: &AngularVelocity, other: &AngularVelocity, t: f32) -> AngularVelocity {
        let res = AngularVelocity(start.0 * (1.0 - t) + other.0 * t);
        trace!(
            "angular velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }
}

pub mod position_rotation {
    use bevy_math::Quat;

    use super::*;

    /// Interpolates `(Position, Rotation, LinearVelocity, AngularVelocity)`
    /// using Hermite interpolation for position and rotation.
    ///
    /// This is intended for [`FrameInterpolate`](lightyear_core::prelude::FrameInterpolate)
    /// and prediction correction in
    /// [`AvianReplicationMode::Position`](crate::plugin::AvianReplicationMode::Position).
    /// Avian velocities are expressed per second, so
    /// [`InterpolationSampleContext::sample_delta_secs`] scales the endpoint
    /// tangents to the bracketing sample interval. If that interval is
    /// unavailable, position falls back to linear interpolation and rotation
    /// falls back to shortest-path interpolation.
    ///
    /// [`LightyearAvianPlugin`](crate::plugin::LightyearAvianPlugin)
    /// automatically registers this as a full bundle rule in
    /// [`AvianReplicationMode::Position`](crate::plugin::AvianReplicationMode::Position).
    pub fn hermite(
        start: (Position, Rotation, LinearVelocity, AngularVelocity),
        end: (Position, Rotation, LinearVelocity, AngularVelocity),
        ctx: InterpolationSampleContext,
    ) -> (Position, Rotation, LinearVelocity, AngularVelocity) {
        let (position, rotation) = if let Some(sample_delta_secs) = ctx.sample_delta_secs {
            let t = Real::from(ctx.t);
            let t2 = t * t;
            let t3 = t2 * t;
            let dt = Real::from(sample_delta_secs);
            let position = start.0.0 * (2.0 * t3 - 3.0 * t2 + 1.0)
                + end.0.0 * (3.0 * t2 - 2.0 * t3)
                + (start.2.0.real() * dt) * (t3 - 2.0 * t2 + t)
                + (end.2.0.real() * dt) * (t3 - t2);
            let rotation = hermite_quat(
                Quat::from(start.1),
                Quat::from(end.1),
                Vec3::Z * start.3.0 * sample_delta_secs,
                Vec3::Z * end.3.0 * sample_delta_secs,
                ctx.t,
                true,
            );
            (Position(position), Rotation::from(rotation))
        } else {
            (
                super::position::lerp(&start.0, &end.0, ctx.t),
                super::rotation::lerp(&start.1, &end.1, ctx.t),
            )
        };

        (
            position,
            rotation,
            super::linear_velocity::lerp(&start.2, &end.2, ctx.t),
            super::angular_velocity::lerp(&start.3, &end.3, ctx.t),
        )
    }
}

pub mod transform {
    use super::*;

    /// Interpolates `(Transform, AngularVelocity, LinearVelocity)` using
    /// Hermite interpolation for translation and rotation.
    ///
    /// Avian velocities are expressed per second, so this function uses
    /// [`InterpolationSampleContext::sample_delta_secs`] to scale the endpoint
    /// velocities to the bracketing sample interval. If the sample interval is
    /// unavailable, it falls back to linear translation/scale interpolation and
    /// spherical rotation interpolation.
    ///
    /// Bundle interpolation samples all three histories at shared ticks;
    /// unchanged components carry their latest values forward. The returned
    /// velocity components are linearly interpolated over the same fraction.
    ///
    /// Register it as a higher-priority bundle rule when Transform-mode
    /// smoothing should use velocity-aware Hermite interpolation:
    ///
    /// ```rust,ignore
    /// app.interpolate_bundle_with_priority::<(Transform, AngularVelocity, LinearVelocity)>(
    ///     100,
    ///     InterpolationFns::interpolate_with_context(
    ///         lightyear_avian2d::types::transform::hermite,
    ///     ),
    /// );
    /// ```
    pub fn hermite(
        start: (Transform, AngularVelocity, LinearVelocity),
        end: (Transform, AngularVelocity, LinearVelocity),
        ctx: InterpolationSampleContext,
    ) -> (Transform, AngularVelocity, LinearVelocity) {
        let transform = if let Some(sample_delta_secs) = ctx.sample_delta_secs {
            let start_linear_velocity = start.2.0.extend(0.0) * sample_delta_secs;
            let end_linear_velocity = end.2.0.extend(0.0) * sample_delta_secs;
            let start_angular_velocity = Vec3::Z * start.1.0 * sample_delta_secs;
            let end_angular_velocity = Vec3::Z * end.1.0 * sample_delta_secs;
            Transform {
                translation: hermite_vec3(
                    start.0.translation,
                    end.0.translation,
                    start_linear_velocity,
                    end_linear_velocity,
                    ctx.t,
                ),
                rotation: hermite_quat(
                    start.0.rotation,
                    end.0.rotation,
                    start_angular_velocity,
                    end_angular_velocity,
                    ctx.t,
                    true,
                ),
                scale: start.0.scale.lerp(end.0.scale, ctx.t),
            }
        } else {
            linear(start.0, end.0, ctx.t)
        };

        (
            transform,
            super::angular_velocity::lerp(&start.1, &end.1, ctx.t),
            super::linear_velocity::lerp(&start.2, &end.2, ctx.t),
        )
    }

    fn linear(start: Transform, end: Transform, t: f32) -> Transform {
        Transform {
            translation: start.translation.lerp(end.translation, t),
            rotation: start.rotation.slerp(end.rotation, t),
            scale: start.scale.lerp(end.scale, t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian2d::math::RVector;
    use bevy_math::{Quat, Vec2};

    fn state(
        transform: Transform,
        angular_velocity: f32,
        linear_velocity: (f32, f32),
    ) -> (Transform, AngularVelocity, LinearVelocity) {
        (
            transform,
            AngularVelocity(angular_velocity),
            LinearVelocity(Vec2::new(linear_velocity.0, linear_velocity.1)),
        )
    }

    fn position_rotation_state(
        position: (f32, f32),
        rotation: f32,
        linear_velocity: (f32, f32),
        angular_velocity: f32,
    ) -> (Position, Rotation, LinearVelocity, AngularVelocity) {
        (
            Position(RVector::new(Real::from(position.0), Real::from(position.1))),
            Rotation::radians(rotation),
            LinearVelocity(Vec2::new(linear_velocity.0, linear_velocity.1)),
            AngularVelocity(angular_velocity),
        )
    }

    fn assert_vec3_close(actual: Vec3, expected: Vec3) {
        assert!(
            actual.distance(expected) <= 1e-5,
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn assert_quat_close(actual: Quat, expected: Quat) {
        assert!(
            actual.dot(expected).abs() >= 1.0 - 1e-5,
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn hermite_preserves_endpoints() {
        let start = state(
            Transform::from_xyz(1.0, 2.0, 0.0)
                .with_rotation(Quat::from_rotation_z(0.25))
                .with_scale(Vec3::splat(2.0)),
            3.0,
            (4.0, 5.0),
        );
        let end = state(
            Transform::from_xyz(6.0, 7.0, 0.0)
                .with_rotation(Quat::from_rotation_z(1.25))
                .with_scale(Vec3::splat(4.0)),
            -2.0,
            (8.0, 9.0),
        );

        let at_start =
            transform::hermite(start, end, InterpolationSampleContext::new(0.0, Some(0.5)));
        let at_end =
            transform::hermite(start, end, InterpolationSampleContext::new(1.0, Some(0.5)));

        assert_vec3_close(at_start.0.translation, start.0.translation);
        assert_quat_close(at_start.0.rotation, start.0.rotation);
        assert_vec3_close(at_start.0.scale, start.0.scale);
        assert_vec3_close(at_end.0.translation, end.0.translation);
        assert_quat_close(at_end.0.rotation, end.0.rotation);
        assert_vec3_close(at_end.0.scale, end.0.scale);
    }

    #[test]
    fn hermite_scales_velocity_by_sample_interval() {
        let start = state(Transform::default(), 0.0, (4.0, 0.0));
        let end = state(Transform::default(), 0.0, (0.0, 0.0));

        let one_second =
            transform::hermite(start, end, InterpolationSampleContext::new(0.5, Some(1.0)));
        let two_seconds =
            transform::hermite(start, end, InterpolationSampleContext::new(0.5, Some(2.0)));

        assert_vec3_close(one_second.0.translation, Vec3::X * 0.5);
        assert_vec3_close(two_seconds.0.translation, Vec3::X);
    }

    #[test]
    fn hermite_unwraps_full_revolution() {
        let start = state(Transform::default(), core::f32::consts::TAU, (0.0, 0.0));
        let end = start;
        let midpoint =
            transform::hermite(start, end, InterpolationSampleContext::new(0.5, Some(1.0)));

        assert_vec3_close(midpoint.0.rotation * Vec3::X, -Vec3::X);
    }

    #[test]
    fn hermite_without_sample_interval_falls_back_to_linear() {
        let start = state(Transform::default(), 0.0, (100.0, 0.0));
        let end_transform = Transform::from_xyz(8.0, 4.0, 0.0)
            .with_rotation(Quat::from_rotation_z(core::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::splat(3.0));
        let end = state(end_transform, 0.0, (0.0, 0.0));
        let result = transform::hermite(start, end, InterpolationSampleContext::from_t(0.25));

        assert_vec3_close(
            result.0.translation,
            start.0.translation.lerp(end.0.translation, 0.25),
        );
        assert_quat_close(
            result.0.rotation,
            start.0.rotation.slerp(end.0.rotation, 0.25),
        );
        assert_vec3_close(result.0.scale, start.0.scale.lerp(end.0.scale, 0.25));
    }

    #[test]
    fn position_rotation_hermite_uses_velocity_tangents() {
        let start = position_rotation_state((0.0, 0.0), 0.0, (4.0, 0.0), core::f32::consts::TAU);
        let end = position_rotation_state((0.0, 0.0), 0.0, (0.0, 0.0), core::f32::consts::TAU);

        let midpoint =
            position_rotation::hermite(start, end, InterpolationSampleContext::new(0.5, Some(1.0)));

        assert_vec3_close(midpoint.0.0.f32().extend(0.0), Vec3::X * 0.5);
        assert_vec3_close(Quat::from(midpoint.1) * Vec3::X, -Vec3::X);
        assert_vec3_close(midpoint.2.0.extend(0.0), Vec3::X * 2.0);
        let angular_velocity = midpoint.3.0;
        assert!((angular_velocity - core::f32::consts::TAU).abs() <= 1e-5);
    }

    #[test]
    fn position_rotation_hermite_without_interval_falls_back_to_lerp() {
        let start = position_rotation_state((0.0, 0.0), 0.0, (100.0, 0.0), 10.0);
        let end =
            position_rotation_state((8.0, 4.0), core::f32::consts::FRAC_PI_2, (0.0, 0.0), 0.0);

        let result =
            position_rotation::hermite(start, end, InterpolationSampleContext::from_t(0.25));

        assert_vec3_close(result.0.0.f32().extend(0.0), Vec3::new(2.0, 1.0, 0.0));
        assert_vec3_close(
            Quat::from(result.1) * Vec3::X,
            Quat::from_rotation_z(core::f32::consts::FRAC_PI_8) * Vec3::X,
        );
    }

    #[test]
    fn position_rotation_hermite_preserves_world_position_precision() {
        let base = Real::from(16_777_216.0_f32);
        let start = (
            Position(RVector::X * (base + 1.0)),
            Rotation::default(),
            LinearVelocity::default(),
            AngularVelocity::default(),
        );
        let end = (
            Position(RVector::X * (base + 5.0)),
            start.1,
            start.2,
            start.3,
        );
        for (t, expected) in [
            (0.0, start.0),
            (0.5, Position((start.0.0 + end.0.0) * 0.5)),
            (1.0, end.0),
        ] {
            let sample = position_rotation::hermite(
                start,
                end,
                InterpolationSampleContext::new(t, Some(1.0)),
            );
            assert_eq!(sample.0, expected);
        }
    }

    #[test]
    fn velocity_diff_roundtrip() {
        use lightyear_replication::diffable::Diffable;

        let start = LinearVelocity(Vec2::new(1.0_f32, -2.0_f32));
        let end = LinearVelocity(Vec2::new(8.0_f32, 0.5_f32));
        assert_eq!(start.diff(&start), LinearVelocity::default());
        let mut value = start;
        value.apply_diff(&start.diff(&end));
        assert_eq!(value, end);

        let start = AngularVelocity(0.5_f32);
        let end = AngularVelocity(-4.0_f32);
        assert_eq!(start.diff(&start), AngularVelocity::default());
        let mut value = start;
        value.apply_diff(&start.diff(&end));
        assert_eq!(value, end);
    }
}
