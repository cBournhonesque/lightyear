use crate::delta::Diffable;
use bevy_math::{Isometry2d, Isometry3d, Quat, Rot2, Vec3};
use bevy_transform::prelude::Transform;

#[cfg(feature = "avian2d")]
mod avian2d;

#[cfg(feature = "avian3d")]
mod avian3d;

impl Diffable<Isometry2d> for Transform {
    fn base_value() -> Self {
        Transform::default()
    }

    fn diff(&self, new: &Self) -> Isometry2d {
        let translation_diff = new.translation.truncate() - self.translation.truncate();
        // Extract Z rotation directly from quaternions
        let (z1, w1) = (self.rotation.z, self.rotation.w);
        let (z2, w2) = (new.rotation.z, new.rotation.w);

        // Compute rotation delta angle efficiently
        // angle_new - angle_self = atan2(sin, cos) of relative quaternion
        let rotation_diff = 2.0 * ((z2 * w1 - w2 * z1).atan2(w2 * w1 + z2 * z1));
        Isometry2d {
            translation: translation_diff,
            rotation: Rot2::radians(rotation_diff),
        }
    }

    fn apply_diff(&mut self, delta: &Isometry2d) {
        self.translation.x += delta.translation.x;
        self.translation.y += delta.translation.y;
        let rotation_delta_3d = Quat::from_rotation_z(delta.rotation.as_radians());
        self.rotation *= rotation_delta_3d;
    }
}

impl Diffable<Isometry3d> for Transform {
    fn base_value() -> Self {
        Transform::default()
    }

    fn diff(&self, new: &Self) -> Isometry3d {
        Isometry3d::new(
            new.translation - self.translation,
            self.rotation.inverse() * new.rotation,
        )
    }

    fn apply_diff(&mut self, delta: &Isometry3d) {
        self.translation += Vec3::from(delta.translation);
        self.rotation *= delta.rotation;
    }
}
