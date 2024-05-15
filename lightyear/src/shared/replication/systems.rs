//! Bevy [`bevy::prelude::System`]s used for replication
use std::any::TypeId;
use std::ops::Deref;

use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemChangeTick;
use bevy::prelude::{
    Added, App, Changed, Commands, Component, DetectChanges, Entity, Has, IntoSystemConfigs, Mut,
    PostUpdate, PreUpdate, Query, Ref, RemovedComponents, Res, ResMut, With, Without,
};
use tracing::{debug, error, info, trace, warn};

use crate::prelude::{ClientId, ReplicationGroup, ShouldBePredicted, TargetEntity, TickManager};
use crate::protocol::component::{ComponentNetId, ComponentRegistry};
use crate::serialize::RawData;
use crate::server::replication::send::SyncTarget;
use crate::server::replication::ServerReplicationSet;
use crate::server::visibility::immediate::{ClientVisibility, ReplicateVisibility};
use crate::shared::replication::components::{
    DespawnTracker, DisabledComponent, OverrideTargetComponent, ReplicateOnceComponent,
    ReplicationGroupId, ReplicationTarget, VisibilityMode,
};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::replication::{ReplicationReceive, ReplicationSend};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

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
