//! Bevy [`bevy::prelude::System`]s used for replication

use bevy::prelude::{Res, ResMut};

use crate::prelude::TickManager;
use crate::shared::replication::{ReplicationReceive, ReplicationSend};

/// Systems that runs internal clean-up on the ReplicationSender
/// (handle tick wrapping, etc.)
pub(crate) fn send_cleanup<R: ReplicationSend>(
    mut sender: ResMut<R>,
    tick_manager: Res<TickManager>,
) {
    let tick = tick_manager.tick();
    sender.cleanup(tick);
}

/// Systems that runs internal clean-up on the ReplicationReceiver
/// (handle tick wrapping, etc.)
pub(crate) fn receive_cleanup<R: ReplicationReceive>(
    mut receiver: ResMut<R>,
    tick_manager: Res<TickManager>,
) {
    let tick = tick_manager.tick();
    receiver.cleanup(tick);
}
