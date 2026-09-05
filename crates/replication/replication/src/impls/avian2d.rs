use crate::diffable::Diffable;
use avian2d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};

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

impl Diffable<LinearVelocity> for LinearVelocity {
    fn base_value() -> Self {
        LinearVelocity::default()
    }

    fn diff(&self, new: &Self) -> LinearVelocity {
        LinearVelocity(new.0 - self.0)
    }

    fn apply_diff(&mut self, delta: &LinearVelocity) {
        self.0 += delta.0;
    }
}

impl Diffable<AngularVelocity> for AngularVelocity {
    fn base_value() -> Self {
        AngularVelocity::default()
    }

    fn diff(&self, new: &Self) -> AngularVelocity {
        AngularVelocity(new.0 - self.0)
    }

    fn apply_diff(&mut self, delta: &AngularVelocity) {
        self.0 += delta.0;
    }
}
