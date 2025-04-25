//! When a ReplicationSender first connects to a ReplicationReceiver, it sends a
//! a message to inform the receiver of its SendInterval. This interval is used
//! by the receiver to determine how the InterpolationTime should be configured

use bevy::app::{App, Plugin};
use lightyear_core::tick::TickDuration;
use serde::{Deserialize, Serialize};

// TODO: the message should be a trigger
#[derive(Serialize, Deserialize)]
pub struct SenderMetadata {
    send_interval: TickDuration,
}

pub struct MetadataPlugin;

impl MetadataPlugin {

}


impl Plugin for MetadataPlugin {
    fn build(&self, app: &mut App) {

        todo!()
    }
}