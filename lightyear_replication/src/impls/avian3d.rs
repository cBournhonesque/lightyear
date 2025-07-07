use crate::delta::Diffable;
use avian3d::position::{Position, Rotation};

impl Diffable for Position {
    type Delta = Self;

    fn base_value() -> Self {
        Position::default()
    }

    fn diff(&self, new: &Self) -> Self::Delta {
        Position::from(new.0 - self.0)
    }

    fn apply_diff(&mut self, delta: &Self::Delta) {
        self.0 += **delta;
    }
}

impl Diffable for Rotation {
    type Delta = Self;

    fn base_value() -> Self {
        Rotation::default()
    }

    fn diff(&self, new: &Self) -> Self::Delta {
        Rotation(new.0 * *self.inverse())
    }

    fn apply_diff(&mut self, delta: &Self::Delta) {
        self.0 *= delta.0;
    }
}
