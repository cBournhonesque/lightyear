//! Handles client-side prediction
#![no_std]

extern crate alloc;
extern crate core;
#[cfg(feature = "std")]
extern crate std;

use core::fmt::Debug;

#[allow(unused)]
pub(crate) mod archetypes;
pub mod correction;
pub mod despawn;
pub mod diagnostics;
pub mod manager;
pub mod plugin;
pub mod predicted_history;
pub mod registry;
pub mod resource_history;
pub mod rollback;

mod deterministic;

pub mod prelude {
    pub use crate::Predicted;
    pub use crate::correction::VisualCorrection;
    pub use crate::despawn::{PredictionDespawnCommandsExt, PredictionDisable};
    pub use crate::diagnostics::PredictionMetrics;
    pub use crate::manager::{LastConfirmedInput, PredictionManager, RollbackMode, RollbackPolicy};
    pub use crate::plugin::{PredictionPlugin, PredictionSystems};
    pub use crate::predicted_history::PredictionHistory;
    pub use crate::registry::{
        PredictionAppRegistrationExt, PredictionRegistrationExt, PredictionRegistry,
    };
    pub use crate::rollback::{
        DeterministicPredicted, DisableRollback, DisabledDuringRollback, RollbackSystems,
    };
}

use lightyear_core::tick::Tick;

pub(crate) trait ToTick {
    fn tick(&self) -> Tick;
}

impl ToTick for lightyear_replication::prelude::ConfirmHistory {
    fn tick(&self) -> Tick {
        self.last_tick().get().into()
    }
}

impl ToTick for lightyear_replication::prelude::ServerMutateTicks {
    fn tick(&self) -> Tick {
        self.last_tick().get().into()
    }
}

use bevy_ecs::component::{Component, Mutable};
pub use lightyear_core::prediction::Predicted;

/// Trait for components that can be synchronized between a confirmed entity and its predicted/interpolated counterpart.
///
/// This is a marker trait, requiring `Component<Mutability=Mutable> + Clone + PartialEq`.
/// Components implementing this trait can have their state managed by the prediction and interpolation systems
/// according to the specified `PredictionMode`.
pub trait SyncComponent: Component<Mutability = Mutable> + Clone + PartialEq + Debug {}
impl<T> SyncComponent for T where T: Component<Mutability = Mutable> + Clone + PartialEq + Debug {}
