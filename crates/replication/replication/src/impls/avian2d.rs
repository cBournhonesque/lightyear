use crate::delta::Diffable;
use avian2d::prelude::{Position, Rotation};

impl Diffable<Position> for Position {
    fn base_value() -> Self {
        Position::default()
    }

    fn diff(&self, new: &Self) -> Position {
        Position(new.0 - self.0)
    }

    fn apply_diff(&mut self, delta: &Position) {
        self.0 += **delta;
    }
}

impl Diffable<Rotation> for Rotation {
    fn base_value() -> Self {
        Rotation::default()
    }

    fn diff(&self, new: &Self) -> Rotation {
        Rotation::radians(self.angle_between(*new))
    }

    fn apply_diff(&mut self, delta: &Rotation) {
        *self = self.add_angle_fast(delta.as_radians());
    }
}
