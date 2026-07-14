//! Implement lightyear traits for some common bevy types
use avian3d::math::{AdjustPrecision, AsF32, Scalar};
use avian3d::prelude::*;
use bevy_transform::components::Transform;
use bevy_transform_interpolation::hermite::{hermite_quat, hermite_vec3};
use lightyear_interpolation::prelude::InterpolationSampleContext;
use tracing::trace;

#[cfg(feature = "deterministic")]
use core::hash::Hasher;

pub mod position {
    use super::*;

    pub fn lerp(start: &Position, other: &Position, t: f32) -> Position {
        let u = Scalar::from(t);
        let res = Position::new(start.0 * (1.0 - u) + other.0 * u);
        trace!(
            "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }

    #[cfg(feature = "deterministic")]
    pub fn hash(pos: &Position, hasher: &mut seahash::SeaHasher) {
        hasher.write_u32(pos.x.to_bits());
        hasher.write_u32(pos.y.to_bits());
        hasher.write_u32(pos.z.to_bits());
    }
}

pub mod rotation {
    use super::*;

    /// We want to smoothly interpolate between the two quaternions by default,
    /// rather than using a quicker but less correct linear interpolation.
    pub fn lerp(start: &Rotation, other: &Rotation, t: f32) -> Rotation {
        start.slerp(*other, Scalar::from(t))
    }

    #[cfg(feature = "deterministic")]
    pub fn hash(rot: &Rotation, hasher: &mut seahash::SeaHasher) {
        let [x, y, z, w] = rot.to_array();
        hasher.write_u32(x.to_bits());
        hasher.write_u32(y.to_bits());
        hasher.write_u32(z.to_bits());
        hasher.write_u32(w.to_bits());
    }
}

pub mod linear_velocity {
    use super::*;

    pub fn lerp(start: &LinearVelocity, other: &LinearVelocity, t: f32) -> LinearVelocity {
        let u = Scalar::from(t);
        let res = LinearVelocity(start.0 * (1.0 - u) + other.0 * u);
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
        let u = Scalar::from(t);
        let res = AngularVelocity(start.0 * (1.0 - u) + other.0 * u);
        trace!(
            "angular velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
            start, other, t, res
        );
        res
    }
}

pub mod position_rotation {
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
    /// falls back to spherical interpolation.
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
            let position = hermite_vec3(
                start.0.0.f32(),
                end.0.0.f32(),
                start.2.0.f32() * sample_delta_secs,
                end.2.0.f32() * sample_delta_secs,
                ctx.t,
            );
            let rotation = hermite_quat(
                start.1.0.f32(),
                end.1.0.f32(),
                start.3.0.f32() * sample_delta_secs,
                end.3.0.f32() * sample_delta_secs,
                ctx.t,
                true,
            );
            (
                Position(position.adjust_precision()),
                Rotation::from(rotation),
            )
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
    /// Bundle interpolation requires all three component histories to have the
    /// same bracketing ticks. The returned velocity components are linearly
    /// interpolated over the same fraction.
    ///
    /// Register it as a higher-priority bundle rule when Transform-mode
    /// smoothing should use velocity-aware Hermite interpolation:
    ///
    /// ```rust,ignore
    /// app.interpolate_bundle_with_priority::<(Transform, AngularVelocity, LinearVelocity)>(
    ///     100,
    ///     InterpolationFns::interpolate_with_context(
    ///         lightyear_avian3d::types::transform::hermite,
    ///     ),
    /// );
    /// ```
    pub fn hermite(
        start: (Transform, AngularVelocity, LinearVelocity),
        end: (Transform, AngularVelocity, LinearVelocity),
        ctx: InterpolationSampleContext,
    ) -> (Transform, AngularVelocity, LinearVelocity) {
        let transform = if let Some(sample_delta_secs) = ctx.sample_delta_secs {
            let start_linear_velocity = start.2.0.f32() * sample_delta_secs;
            let end_linear_velocity = end.2.0.f32() * sample_delta_secs;
            let start_angular_velocity = start.1.0.f32() * sample_delta_secs;
            let end_angular_velocity = end.1.0.f32() * sample_delta_secs;
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
    use avian3d::math::Vector;
    use bevy_math::{Quat, Vec3};

    fn state(
        transform: Transform,
        angular_velocity: Vec3,
        linear_velocity: Vec3,
    ) -> (Transform, AngularVelocity, LinearVelocity) {
        (
            transform,
            AngularVelocity(Vector::new(
                Scalar::from(angular_velocity.x),
                Scalar::from(angular_velocity.y),
                Scalar::from(angular_velocity.z),
            )),
            LinearVelocity(Vector::new(
                Scalar::from(linear_velocity.x),
                Scalar::from(linear_velocity.y),
                Scalar::from(linear_velocity.z),
            )),
        )
    }

    fn position_rotation_state(
        position: Vec3,
        rotation: Quat,
        linear_velocity: Vec3,
        angular_velocity: Vec3,
    ) -> (Position, Rotation, LinearVelocity, AngularVelocity) {
        (
            Position(Vector::new(
                Scalar::from(position.x),
                Scalar::from(position.y),
                Scalar::from(position.z),
            )),
            Rotation::from(rotation),
            LinearVelocity(Vector::new(
                Scalar::from(linear_velocity.x),
                Scalar::from(linear_velocity.y),
                Scalar::from(linear_velocity.z),
            )),
            AngularVelocity(Vector::new(
                Scalar::from(angular_velocity.x),
                Scalar::from(angular_velocity.y),
                Scalar::from(angular_velocity.z),
            )),
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
            Transform::from_xyz(1.0, 2.0, 3.0)
                .with_rotation(Quat::from_rotation_y(0.25))
                .with_scale(Vec3::splat(2.0)),
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(4.0, 5.0, 6.0),
        );
        let end = state(
            Transform::from_xyz(6.0, 7.0, 8.0)
                .with_rotation(Quat::from_rotation_y(1.25))
                .with_scale(Vec3::splat(4.0)),
            Vec3::new(-1.0, -2.0, -3.0),
            Vec3::new(8.0, 9.0, 10.0),
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
        let start = state(Transform::default(), Vec3::ZERO, Vec3::X * 4.0);
        let end = state(Transform::default(), Vec3::ZERO, Vec3::ZERO);

        let one_second =
            transform::hermite(start, end, InterpolationSampleContext::new(0.5, Some(1.0)));
        let two_seconds =
            transform::hermite(start, end, InterpolationSampleContext::new(0.5, Some(2.0)));

        assert_vec3_close(one_second.0.translation, Vec3::X * 0.5);
        assert_vec3_close(two_seconds.0.translation, Vec3::X);
    }

    #[test]
    fn hermite_unwraps_full_revolution() {
        let start = state(
            Transform::default(),
            Vec3::Z * core::f32::consts::TAU,
            Vec3::ZERO,
        );
        let end = start;
        let midpoint =
            transform::hermite(start, end, InterpolationSampleContext::new(0.5, Some(1.0)));

        assert_vec3_close(midpoint.0.rotation * Vec3::X, -Vec3::X);
    }

    #[test]
    fn hermite_without_sample_interval_falls_back_to_linear() {
        let start = state(Transform::default(), Vec3::ZERO, Vec3::X * 100.0);
        let end_transform = Transform::from_xyz(8.0, 4.0, 2.0)
            .with_rotation(Quat::from_rotation_y(core::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::splat(3.0));
        let end = state(end_transform, Vec3::ZERO, Vec3::ZERO);
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
        let start = position_rotation_state(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::X * 4.0,
            Vec3::Z * core::f32::consts::TAU,
        );
        let end = position_rotation_state(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::ZERO,
            Vec3::Z * core::f32::consts::TAU,
        );

        let midpoint =
            position_rotation::hermite(start, end, InterpolationSampleContext::new(0.5, Some(1.0)));

        assert_vec3_close(midpoint.0.0.f32(), Vec3::X * 0.5);
        assert_vec3_close(midpoint.1.0.f32() * Vec3::X, -Vec3::X);
        assert_vec3_close(midpoint.2.0.f32(), Vec3::X * 2.0);
        assert_vec3_close(midpoint.3.0.f32(), Vec3::Z * core::f32::consts::TAU);
    }

    #[test]
    fn position_rotation_hermite_without_interval_falls_back_to_lerp() {
        let start =
            position_rotation_state(Vec3::ZERO, Quat::IDENTITY, Vec3::X * 100.0, Vec3::Y * 10.0);
        let end = position_rotation_state(
            Vec3::new(8.0, 4.0, 2.0),
            Quat::from_rotation_y(core::f32::consts::FRAC_PI_2),
            Vec3::ZERO,
            Vec3::ZERO,
        );

        let result =
            position_rotation::hermite(start, end, InterpolationSampleContext::from_t(0.25));

        assert_vec3_close(result.0.0.f32(), Vec3::new(2.0, 1.0, 0.5));
        assert_vec3_close(
            result.1.0.f32() * Vec3::X,
            Quat::from_rotation_y(core::f32::consts::FRAC_PI_8) * Vec3::X,
        );
    }
}
