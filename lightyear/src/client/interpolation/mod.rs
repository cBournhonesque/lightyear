//! Handles interpolation of entities between server updates
use std::ops::{Add, Mul};

use bevy::prelude::{Added, Commands, Component, Entity, Query, ResMut};
use tracing::info;

pub use interpolate::InterpolateStatus;
pub use interpolation_history::ConfirmedHistory;
pub use plugin::{add_interpolation_systems, add_prepare_interpolation_systems};

use crate::client::components::{ComponentSyncMode, Confirmed, SyncComponent};
use crate::client::interpolation::despawn::InterpolationMapping;
use crate::shared::replication::components::ShouldBeInterpolated;

mod despawn;
mod interpolate;
pub mod interpolation_history;
pub mod plugin;

/// This module handles doing snapshot interpolations for entities controlled by other clients.
///
/// We want to receive smooth updates for the other players' actions
/// But we receive their actions at a given timestep that might not match the physics timestep.

/// Which means we can do one of two things:
/// - apply client prediction for all players
/// - apply client prediction for the controlled player, and snapshot interpolation for the other players

// TODO:
// - same thing, add InterpolationTarget on Replicate, which translates into ShouldBeInterpolated.
// - if we see that on a confirmed entity, then we create a Interpolated entity.
// - that entity will keep a component history (along with the ticks), and we will interpolate between the last 2 components.
// - re-use component history ?

// TODO: maybe merge this with PredictedComponent?
//  basically it is a HistoryComponent. And we can have modes for Prediction or Interpolation

pub trait InterpFn<C> {
    fn lerp(start: C, other: C, t: f32) -> C;
}

pub struct LinearInterpolation;
impl<C> InterpFn<C> for LinearInterpolation
where
    C: Mul<f32, Output = C> + Add<C, Output = C>,
{
    fn lerp(start: C, other: C, t: f32) -> C {
        start * (1.0 - t) + other * t
    }
}

pub trait InterpolatedComponent<C>: SyncComponent {
    type Fn: InterpFn<C>;

    fn lerp(start: C, other: C, t: f32) -> C {
        Self::Fn::lerp(start, other, t)
    }
}

// pub trait InterpolatedComponent<C>: SyncComponent + Sized {
//     type Fn: InterpFn<C>;
//     /// Which interpolation function to use
//     /// By default, it will be a linear interpolation
//     fn lerp_mode() -> LerpMode<Self>;
//
//     fn lerp_linear(start: Self, other: Self, t: f32) -> Self
//     where
//         Self: Mul<f32, Output = Self> + Add<Self, Output = Self>,
//     {
//         start * (1.0 - t) + other * t
//     }
//
//     fn lerp_custom(start: Self, other: Self, t: f32, lerp: fn(Self, Self, f32) -> Self) -> Self {
//         lerp(start, other, t)
//     }
// }

// #[derive(Debug)]
// pub enum LerpMode<C: InterpolatedComponent> {
//     Linear,
//     // TODO: change this to a trait object?
//     Custom(fn(C, C, f32) -> C),
// }

/// Marks an entity that is being interpolated by the client
#[derive(Component, Debug)]
pub struct Interpolated {
    // TODO: maybe here add an interpolation function?
    pub confirmed_entity: Entity,
    // TODO: add config about despawn behaviour here:
    //  - despawn immediately all components
    //  - leave the entity alive until the confirmed entity catches up to it and then it gets removed.
    //    - or do this only for certain components (audio, animation, particles..) -> mode on PredictedComponent
    // rollback_state: RollbackState,
}

pub fn spawn_interpolated_entity(
    mut commands: Commands,
    mut mapping: ResMut<InterpolationMapping>,
    mut confirmed_entities: Query<(Entity, Option<&mut Confirmed>), Added<ShouldBeInterpolated>>,
) {
    for (confirmed_entity, confirmed) in confirmed_entities.iter_mut() {
        // spawn a new interpolated entity
        let interpolated_entity_mut = commands.spawn(Interpolated { confirmed_entity });
        let interpolated = interpolated_entity_mut.id();

        // add Confirmed to the confirmed entity
        // safety: we know the entity exists
        let mut confirmed_entity_mut = commands.get_entity(confirmed_entity).unwrap();
        mapping
            .confirmed_to_interpolated
            .insert(confirmed_entity, interpolated);
        if let Some(mut confirmed) = confirmed {
            confirmed.interpolated = Some(interpolated);
        } else {
            confirmed_entity_mut.insert(Confirmed {
                interpolated: Some(interpolated),
                predicted: None,
            });
        }
        info!(
            "Spawn interpolated entity {:?} for confirmed: {:?}",
            interpolated, confirmed_entity
        );
        #[cfg(feature = "metrics")]
        {
            metrics::increment_counter!("spawn_interpolated_entity");
        }
    }
}
