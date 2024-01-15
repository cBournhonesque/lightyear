//! Implement lightyear traits for some common bevy types
use crate::_reexport::LinearInterpolator;
use crate::client::components::{ComponentSyncMode, LerpFn, SyncComponent};
use bevy::prelude::Entity;
use bevy::utils::EntityHashSet;
use bevy_xpbd_3d::components::*;
use std::ops::{Add, Mul};
use tracing::{info, trace};

use crate::prelude::{EntityMapper, MapEntities, Message, Named};

pub mod position {
    use super::*;
    impl Named for Position {
        const NAME: &'static str = "Position";
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
}
pub use position::*;

pub mod rotation {
    use bevy::math::Quat;

    use super::*;
    impl Named for Rotation {
        const NAME: &'static str = "Rotation";
    }

    pub struct RotationLinearInterpolation;

    impl LerpFn<Rotation> for RotationLinearInterpolation {
        fn lerp(start: Rotation, other: Rotation, t: f32) -> Rotation {
            /*let shortest_angle =
                ((((other.as_degrees() - start.as_degrees()) % 360.0) + 540.0) % 360.0) - 180.0;
            let res = Rotation::from_degrees(start.as_degrees() + shortest_angle * t);
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
            );*/
            let start_quat: Quat = start.into();
            let other_quat: Quat = other.into();
            let res = Rotation::from(start_quat.lerp(other_quat, t));
            res
        }
    }

    impl<'a> MapEntities<'a> for Rotation {
        fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

        fn entities(&self) -> EntityHashSet<Entity> {
            EntityHashSet::default()
        }
    }
}
pub use rotation::*;

pub mod linear_velocity {
    use super::*;
    impl Named for LinearVelocity {
        const NAME: &'static str = "LinearVelocity";
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
}
pub use linear_velocity::*;

pub mod angular_velocity {
    use super::*;
    impl Named for AngularVelocity {
        const NAME: &'static str = "AngularVelocity";
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
}
pub use angular_velocity::*;

// TODO: in some cases the mass does not change, but in others it doesn't!
//  this is an example of where we don't why to attach the interpolation to the component type,
//  but instead do it per entity?
pub mod mass {
    use super::*;
    impl Named for Mass {
        const NAME: &'static str = "Mass";
    }
    impl<'a> MapEntities<'a> for Mass {
        fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}

        fn entities(&self) -> EntityHashSet<Entity> {
            EntityHashSet::default()
        }
    }
}
pub use mass::*;
