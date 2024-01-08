//! Implement lightyear traits for some common bevy types
use crate::_reexport::LinearInterpolator;
use crate::client::components::{ComponentSyncMode, LerpFn, SyncComponent};
use bevy::prelude::Entity;
use bevy::utils::EntityHashSet;
use bevy_xpbd_2d::components::*;
use std::ops::{Add, Mul};
use tracing::{info, trace};

use crate::prelude::{EntityMapper, MapEntities, Message, Named};

pub mod position {
    use super::*;
    impl Named for Position {
        fn name(&self) -> &'static str {
            "Position"
        }
    }

    pub struct PositionLinearInterpolation;

    impl LerpFn<Position> for PositionLinearInterpolation {
        fn lerp(start: Position, other: Position, t: f32) -> Position {
            let res = Position::new(start.0 * (1.0 - t) + other.0 * t);
            trace!(
                "position lerp: start: {:?} end: {:?} t: {} res: {:?}",
                start,
                other,
                t,
                res
            );
            res
        }
    }

    impl<'a> MapEntities<'a> for Position {
        fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

        fn entities(&self) -> EntityHashSet<Entity> {
            EntityHashSet::default()
        }
    }

    impl Message for Position {}
}
pub use position::*;

pub mod rotation {
    use super::*;
    impl Named for Rotation {
        fn name(&self) -> &'static str {
            "Rotation"
        }
    }

    pub struct RotationLinearInterpolation;

    impl LerpFn<Rotation> for RotationLinearInterpolation {
        fn lerp(start: Rotation, other: Rotation, t: f32) -> Rotation {
            let res =
                Rotation::from_degrees(start.as_degrees() * (1.0 - t) + other.as_degrees() * t);
            trace!(
                "rotation lerp: start: {:?} end: {:?} t: {} res: {:?}",
                start,
                other,
                t,
                res
            );
            res
        }
    }

    impl<'a> MapEntities<'a> for Rotation {
        fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

        fn entities(&self) -> EntityHashSet<Entity> {
            EntityHashSet::default()
        }
    }

    impl Message for Rotation {}
}
pub use rotation::*;

pub mod linear_velocity {
    use super::*;
    impl Named for LinearVelocity {
        fn name(&self) -> &'static str {
            "LinearVelocity"
        }
    }

    pub struct LinearVelocityLinearInterpolation;

    impl LerpFn<LinearVelocity> for LinearVelocityLinearInterpolation {
        fn lerp(start: LinearVelocity, other: LinearVelocity, t: f32) -> LinearVelocity {
            let res = LinearVelocity(start.0 * (1.0 - t) + other.0 * t);
            trace!(
                "linear velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
                start,
                other,
                t,
                res
            );
            res
        }
    }

    impl<'a> MapEntities<'a> for LinearVelocity {
        fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

        fn entities(&self) -> EntityHashSet<Entity> {
            EntityHashSet::default()
        }
    }

    impl Message for LinearVelocity {}
}
pub use linear_velocity::*;

pub mod angular_velocity {
    use super::*;
    impl Named for AngularVelocity {
        fn name(&self) -> &'static str {
            "AngularVelocity"
        }
    }

    pub struct AngularVelocityLinearInterpolation;

    impl LerpFn<AngularVelocity> for AngularVelocityLinearInterpolation {
        fn lerp(start: AngularVelocity, other: AngularVelocity, t: f32) -> AngularVelocity {
            let res = AngularVelocity(start.0 * (1.0 - t) + other.0 * t);
            trace!(
                "angular velocity lerp: start: {:?} end: {:?} t: {} res: {:?}",
                start,
                other,
                t,
                res
            );
            res
        }
    }

    impl<'a> MapEntities<'a> for AngularVelocity {
        fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

        fn entities(&self) -> EntityHashSet<Entity> {
            EntityHashSet::default()
        }
    }

    impl Message for AngularVelocity {}
}
pub use angular_velocity::*;

// TODO: in some cases the mass does not change, but in others it doesn't!
//  this is an example of where we don't why to attach the interpolation to the component type,
//  but instead do it per entity?
pub mod mass {
    use super::*;
    impl Named for Mass {
        fn name(&self) -> &'static str {
            "Mass"
        }
    }
    impl<'a> MapEntities<'a> for Mass {
        fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

        fn entities(&self) -> EntityHashSet<Entity> {
            EntityHashSet::default()
        }
    }

    impl Message for Mass {}
}
pub use mass::*;
