//! Logic related to delta compression (sending only the changes between two states, instead of the new state)

use crate::prelude::Message;
use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::any::Any;

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Copy, Debug)]
pub enum DeltaType {
    /// This delta is computed from a previous value
    Normal,
    /// This delta is computed from the Base value
    FromBase,
}

/// A message that contains a delta between two states (for serializing delta compression)
#[derive(Encode, Decode, Deserialize, Serialize)]
pub struct DeltaMessage<M> {
    pub(crate) delta_type: DeltaType,
    #[bitcode(with_serde)]
    pub(crate) delta: M,
}

/// A type is Diffable when you can:
/// - Compute the delta between two states
/// - Apply the delta to an old state to get the new state
pub trait Diffable: Clone {
    /// The type of the delta between two states
    type Delta: Message;

    /// For the first message (when there is no diff possible), instead of sending the full state
    /// we can compute a delta compared to the `Base` default state
    fn base_value() -> Self;

    /// Compute the delta between two states
    fn diff(&self, other: &Self) -> Self::Delta;

    /// Apply a delta to the current state to reach the new state
    fn apply_diff(&mut self, delta: &Self::Delta);
}
