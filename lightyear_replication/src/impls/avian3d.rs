use crate::delta::Diffable;
use avian3d::prelude::{Position, Rotation};

impl Diffable<Self> for Position {
    fn base_value() -> Self {
        Position::default()
    }

    fn diff(&self, new: &Self) -> Self {
        Position::from(new.0 - self.0)
    }

    fn apply_diff(&mut self, delta: &Self) {
        self.0 += **delta;
    }
}

impl Diffable<Self> for Rotation {
    fn base_value() -> Self {
        Rotation::default()
    }

    fn diff(&self, new: &Self) -> Self {
        Rotation(new.0 * *self.inverse())
    }

    fn apply_diff(&mut self, delta: &Self) {
        self.0 *= delta.0;
    }
}
