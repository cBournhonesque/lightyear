use crate::client::config::ClientConfig;
use bevy::prelude::{Res, SystemSet};

pub mod native;

#[cfg_attr(docsrs, doc(cfg(feature = "leafwing")))]
#[cfg(feature = "leafwing")]
pub mod leafwing;

/// Returns true if there is input delay present
pub fn is_input_delay(config: Res<ClientConfig>) -> bool {
    config.prediction.minimum_input_delay_ticks > 0
        || config.prediction.maximum_input_delay_before_prediction > 0
        || config.prediction.maximum_predicted_ticks < 30
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    // PRE UPDATE
    /// Add any buffer (InputBuffer, ActionDiffBuffer) to newly spawned entities
    AddBuffers,
    /// Receive the InputMessage from other clients
    ReceiveInputMessages,
    // FIXED PRE UPDATE
    /// System Set where the user should emit InputEvents, they will be buffered in the InputBuffers in the BufferClientInputs set.
    /// (For Leafwing, there is nothing to do because the ActionState is updated by leafwing)
    WriteClientInputs,
    /// System Set where we update the ActionState and the InputBuffers
    /// - no rollback: we write the ActionState to the InputBuffers
    /// - rollback: we fetch the ActionState value from the InputBuffers
    BufferClientInputs,

    // FIXED POST UPDATE
    /// Prepare a message for the server with the current tick's inputs.
    /// (we do this in the FixedUpdate schedule because if the simulation is slow (e.g. 10Hz)
    /// we don't want to send an InputMessage every frame)
    PrepareInputMessage,

    // POST UPDATE
    /// System Set to prepare the input message
    SendInputMessage,
    /// Clean up old values to prevent the buffers from growing indefinitely
    CleanUp,
}