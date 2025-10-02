use crate::delta::Diffable;
use avian2d::prelude::{Position, Rotation};

impl Diffable for Position {
    type Delta = Position;

    fn base_value() -> Self {
        Position::default()
    }

    fn diff(&self, new: &Self) -> Self::Delta {
        Position(new.0 - self.0)
    }

    fn apply_diff(&mut self, delta: &Self::Delta) {
        self.0 += **delta;
    }
}

impl Diffable for Rotation {
    type Delta = Rotation;

    fn base_value() -> Self {
        Rotation::default()
    }

    fn diff(&self, new: &Self) -> Self::Delta {
        Rotation::radians(self.angle_between(*new))
    }

    fn apply_diff(&mut self, delta: &Self::Delta) {
        *self = self.add_angle_fast(delta.as_radians());
    }
}
