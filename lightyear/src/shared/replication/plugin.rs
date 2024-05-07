use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy::utils::Duration;

use crate::prelude::{
    NetworkTarget, PrePredicted, RemoteEntityMap, ReplicationGroup, ReplicationMode,
    ShouldBePredicted,
};
use crate::shared::replication::components::{
    PerComponentReplicationMetadata, Replicate, ReplicationGroupId, ReplicationGroupIdBuilder,
    ShouldBeInterpolated, TargetEntity,
};
use crate::shared::replication::entity_map::{InterpolatedEntityMap, PredictedEntityMap};
use crate::shared::replication::hierarchy::{HierarchyReceivePlugin, HierarchySendPlugin};
use crate::shared::replication::resources::{
    receive::ResourceReceivePlugin, send::ResourceSendPlugin,
};
use crate::shared::replication::systems::{add_replication_send_systems, cleanup};
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet, MainSet};

pub(crate) struct ReplicationPlugin<R: ReplicationSend> {
    tick_duration: Duration,
    enable_send: bool,
    enable_receive: bool,
    _marker: std::marker::PhantomData<R>,
}

impl<R: ReplicationSend> ReplicationPlugin<R> {
    pub(crate) fn new(tick_duration: Duration, enable_send: bool, enable_receive: bool) -> Self {
        Self {
            tick_duration,
            enable_send,
            enable_receive,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<R: ReplicationSend> Plugin for ReplicationPlugin<R> {
    fn build(&self, app: &mut App) {
        // TODO: have a better constant for clean_interval?
        let clean_interval = self.tick_duration * (i16::MAX as u32 / 3);

        // REFLECTION
        app.register_type::<Replicate>()
            .register_type::<TargetEntity>()
            .register_type::<PerComponentReplicationMetadata>()
            .register_type::<ReplicationGroupIdBuilder>()
            .register_type::<ReplicationGroup>()
            .register_type::<ReplicationGroupId>()
            .register_type::<ReplicationMode>()
            .register_type::<NetworkTarget>()
            .register_type::<ShouldBeInterpolated>()
            .register_type::<PrePredicted>()
            .register_type::<ShouldBePredicted>()
            .register_type::<RemoteEntityMap>()
            .register_type::<PredictedEntityMap>()
            .register_type::<InterpolatedEntityMap>();

        // TODO: should we put this back into enable_receive?
        app.add_plugins(ResourceReceivePlugin::<R>::default());
        app.add_plugins(ResourceSendPlugin::<R>::default());
        // SYSTEM SETS //
        if self.enable_receive {
            // PLUGINS
            app.add_plugins(HierarchyReceivePlugin::<R>::default());
            // app.add_plugins(ResourceReceivePlugin::<R>::default());
        }
        if self.enable_send {
            app.configure_sets(
                PostUpdate,
                (
                    InternalMainSet::<R::SetMarker>::SendPackets.in_set(MainSet::SendPackets),
                    InternalMainSet::<R::SetMarker>::Send.in_set(MainSet::Send),
                ),
            );
            // NOTE: it's ok to run the replication systems less frequently than every frame
            //  because bevy's change detection detects changes since the last time the system ran (not since the last frame)
            app.configure_sets(
                PostUpdate,
                (
                    (
                        InternalReplicationSet::<R::SetMarker>::HandleReplicateUpdate,
                        InternalReplicationSet::<R::SetMarker>::BufferResourceUpdates,
                        InternalReplicationSet::<R::SetMarker>::Buffer,
                    )
                        .in_set(InternalReplicationSet::<R::SetMarker>::All),
                    (
                        InternalReplicationSet::<R::SetMarker>::BufferEntityUpdates,
                        InternalReplicationSet::<R::SetMarker>::BufferComponentUpdates,
                        InternalReplicationSet::<R::SetMarker>::BufferDespawnsAndRemovals,
                    )
                        .in_set(InternalReplicationSet::<R::SetMarker>::Buffer),
                    (
                        InternalReplicationSet::<R::SetMarker>::BufferEntityUpdates,
                        InternalReplicationSet::<R::SetMarker>::BufferResourceUpdates,
                        InternalReplicationSet::<R::SetMarker>::BufferComponentUpdates,
                        // TODO: verify this, why does handle-replicate-update need to run every frame?
                        //  because Removed<Replicate> is cleared every frame?
                        // NOTE: HandleReplicateUpdate should also run every frame?
                        // NOTE: BufferDespawnsAndRemovals is not in MainSet::Send because we need to run them every frame
                    )
                        .in_set(InternalMainSet::<R::SetMarker>::Send),
                    (
                        InternalReplicationSet::<R::SetMarker>::HandleReplicateUpdate,
                        InternalReplicationSet::<R::SetMarker>::Buffer,
                        InternalMainSet::<R::SetMarker>::SendPackets,
                    )
                        .chain(),
                    (
                        InternalReplicationSet::<R::SetMarker>::BufferResourceUpdates,
                        InternalMainSet::<R::SetMarker>::SendPackets,
                    )
                        .chain(),
                ),
            );
            // SYSTEMS
            add_replication_send_systems::<R>(app);
            // PLUGINS
            app.add_plugins(HierarchySendPlugin::<R>::default());
            // app.add_plugins(ResourceSendPlugin::<R>::default());
        }

        // TODO: split receive cleanup from send cleanup
        // cleanup is for both receive and send
        app.add_systems(Last, cleanup::<R>.run_if(on_timer(clean_interval)));
    }
}
