use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy::utils::Duration;

use crate::_reexport::{ComponentProtocol, ReplicationSend, ShouldBeInterpolated};
use crate::prelude::{
    NetworkTarget, PrePredicted, Protocol, RemoteEntityMap, ReplicationGroup, ReplicationMode,
    ShouldBePredicted,
};
use crate::shared::replication::components::{
    PerComponentReplicationMetadata, Replicate, ReplicationGroupId, ReplicationGroupIdBuilder,
};
use crate::shared::replication::entity_map::{InterpolatedEntityMap, PredictedEntityMap};
use crate::shared::replication::hierarchy::{HierarchyReceivePlugin, HierarchySendPlugin};
use crate::shared::replication::resources::{
    receive::ResourceReceivePlugin, send::ResourceSendPlugin,
};
use crate::shared::replication::systems::{add_replication_send_systems, cleanup};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet, MainSet};

pub(crate) struct ReplicationPlugin<P: Protocol, R: ReplicationSend<P>> {
    tick_duration: Duration,
    enable_send: bool,
    enable_receive: bool,
    _marker: std::marker::PhantomData<(P, R)>,
}

impl<P: Protocol, R: ReplicationSend<P>> ReplicationPlugin<P, R> {
    pub(crate) fn new(tick_duration: Duration, enable_send: bool, enable_receive: bool) -> Self {
        Self {
            tick_duration,
            enable_send,
            enable_receive,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, R: ReplicationSend<P>> Plugin for ReplicationPlugin<P, R> {
    fn build(&self, app: &mut App) {
        // TODO: have a better constant for clean_interval?
        let clean_interval = self.tick_duration * (i16::MAX as u32 / 3);

        // REFLECTION
        app.register_type::<Replicate<P>>()
            .register_type::<P::ComponentKinds>()
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

        // SYSTEM SETS //
        if self.enable_receive {
            app.configure_sets(
                PreUpdate,
                InternalMainSet::<R::SetMarker>::Receive.in_set(MainSet::Receive),
            );
            // PLUGINS
            app.add_plugins(HierarchyReceivePlugin::<P, R>::default());
            app.add_plugins(ResourceReceivePlugin::<P, R>::default());
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
                        InternalReplicationSet::<R::SetMarker>::SendEntityUpdates,
                        InternalReplicationSet::<R::SetMarker>::SendResourceUpdates,
                        InternalReplicationSet::<R::SetMarker>::SendComponentUpdates,
                        InternalReplicationSet::<R::SetMarker>::SendDespawnsAndRemovals,
                    )
                        .in_set(InternalReplicationSet::<R::SetMarker>::All),
                    (
                        InternalReplicationSet::<R::SetMarker>::SendEntityUpdates,
                        InternalReplicationSet::<R::SetMarker>::SendResourceUpdates,
                        InternalReplicationSet::<R::SetMarker>::SendComponentUpdates,
                        // NOTE: SendDespawnsAndRemovals is not in MainSet::Send because we need to run them every frame
                        InternalMainSet::<R::SetMarker>::SendPackets,
                    )
                        .in_set(InternalMainSet::<R::SetMarker>::Send),
                    (
                        InternalReplicationSet::<R::SetMarker>::All,
                        InternalMainSet::<R::SetMarker>::SendPackets,
                    )
                        .chain(),
                ),
            );
            // SYSTEMS
            add_replication_send_systems::<P, R>(app);
            P::Components::add_per_component_replication_send_systems::<R>(app);
            // PLUGINS
            app.add_plugins(HierarchySendPlugin::<P, R>::default());
            app.add_plugins(ResourceSendPlugin::<P, R>::default());
        }

        // TODO: split receive cleanup from send cleanup
        // cleanup is for both receive and send
        app.add_systems(Last, cleanup::<P, R>.run_if(on_timer(clean_interval)));
    }
}
