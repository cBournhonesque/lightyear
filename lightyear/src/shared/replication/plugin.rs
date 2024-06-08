//! This module contains the `ReplicationReceivePlugin` and `ReplicationSendPlugin` plugins, which control
//! the replication of entities and resources.
//!
use crate::shared::replication::hierarchy::{HierarchyReceivePlugin, HierarchySendPlugin};
use crate::shared::replication::resources::{
    receive::ResourceReceivePlugin, send::ResourceSendPlugin,
};
use crate::shared::replication::systems;
use crate::shared::replication::{ReplicationReceive, ReplicationSend};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet, MainSet};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy::utils::Duration;

pub(crate) mod receive {
    use super::*;
    pub(crate) struct ReplicationReceivePlugin<R> {
        clean_interval: Duration,
        _marker: std::marker::PhantomData<R>,
    }

    impl<R> ReplicationReceivePlugin<R> {
        pub(crate) fn new(tick_interval: Duration) -> Self {
            Self {
                // TODO: find a better constant for the clean interval?
                clean_interval: tick_interval * (i16::MAX as u32 / 3),
                _marker: std::marker::PhantomData,
            }
        }
    }

    impl<R: ReplicationReceive> Plugin for ReplicationReceivePlugin<R> {
        fn build(&self, app: &mut App) {
            // PLUGINS
            if !app.is_plugin_added::<shared::SharedPlugin>() {
                app.add_plugins(shared::SharedPlugin);
            }
            app.add_plugins(HierarchyReceivePlugin::<R>::default())
                .add_plugins(ResourceReceivePlugin::<R>::default());

            // SYSTEMS
            app.add_systems(
                Last,
                systems::receive_cleanup::<R>.run_if(on_timer(self.clean_interval)),
            );
        }
    }
}

pub(crate) mod send {
    use super::*;
    use crate::prelude::{Replicating, ReplicationGroup, TimeManager};

    pub(crate) struct ReplicationSendPlugin<R> {
        clean_interval: Duration,
        _marker: std::marker::PhantomData<R>,
    }
    impl<R> ReplicationSendPlugin<R> {
        pub(crate) fn new(tick_interval: Duration) -> Self {
            Self {
                // TODO: find a better constant for the clean interval?
                clean_interval: tick_interval * (i16::MAX as u32 / 3),
                _marker: std::marker::PhantomData,
            }
        }

        /// Tick the internal timers of all replication groups.
        fn tick_replication_group_timers(
            time_manager: Res<TimeManager>,
            mut replication_groups: Query<&mut ReplicationGroup, With<Replicating>>,
        ) {
            for mut replication_group in replication_groups.iter_mut() {
                if let Some(send_frequency) = &mut replication_group.send_frequency {
                    send_frequency.tick(time_manager.delta());
                    if send_frequency.finished() {
                        replication_group.should_send = true;
                    }
                }
            }
        }

        /// After we buffer updates, reset all the `should_send` to false
        /// for the replication groups that have a `send_frequency`
        fn update_replication_group_should_send(
            mut replication_groups: Query<&mut ReplicationGroup, With<Replicating>>,
        ) {
            for mut replication_group in replication_groups.iter_mut() {
                if replication_group.send_frequency.is_some() {
                    replication_group.should_send = false;
                }
            }
        }
    }

    impl<R: ReplicationSend> Plugin for ReplicationSendPlugin<R> {
        fn build(&self, app: &mut App) {
            // PLUGINS
            if !app.is_plugin_added::<shared::SharedPlugin>() {
                app.add_plugins(shared::SharedPlugin);
            }
            app.add_plugins(ResourceSendPlugin::<R>::default())
                .add_plugins(HierarchySendPlugin::<R>::default());

            // SETS
            app.configure_sets(
                PostUpdate,
                (
                    InternalMainSet::<R::SetMarker>::SendPackets.in_set(MainSet::SendPackets),
                    InternalMainSet::<R::SetMarker>::Send.in_set(MainSet::Send),
                ),
            );
            app.configure_sets(
                PostUpdate,
                (
                    (
                        InternalReplicationSet::<R::SetMarker>::BeforeBuffer,
                        InternalReplicationSet::<R::SetMarker>::BufferResourceUpdates,
                        InternalReplicationSet::<R::SetMarker>::Buffer,
                        InternalReplicationSet::<R::SetMarker>::AfterBuffer,
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
                        InternalReplicationSet::<R::SetMarker>::AfterBuffer,
                    )
                        .in_set(InternalMainSet::<R::SetMarker>::Send),
                    (
                        (
                            (
                                InternalReplicationSet::<R::SetMarker>::BeforeBuffer,
                                InternalReplicationSet::<R::SetMarker>::Buffer,
                                InternalReplicationSet::<R::SetMarker>::AfterBuffer,
                            )
                                .chain(),
                            InternalReplicationSet::<R::SetMarker>::BufferResourceUpdates,
                        ),
                        InternalMainSet::<R::SetMarker>::SendPackets,
                    )
                        .chain(),
                ),
            );
            // SYSTEMS
            app.add_systems(
                PostUpdate,
                (
                    ReplicationSendPlugin::<R>::tick_replication_group_timers
                        .in_set(InternalReplicationSet::<R::SetMarker>::BeforeBuffer),
                    ReplicationSendPlugin::<R>::update_replication_group_should_send
                        // note that this runs every send_interval
                        .in_set(InternalReplicationSet::<R::SetMarker>::AfterBuffer),
                ),
            );
            app.add_systems(
                Last,
                systems::send_cleanup::<R>.run_if(on_timer(self.clean_interval)),
            );
        }
    }
}

pub(crate) mod shared {
    use crate::client::replication::send::ReplicateToServer;
    use crate::prelude::{
        PrePredicted, RemoteEntityMap, ReplicateHierarchy, Replicated, ReplicationGroup,
        ReplicationTarget, ShouldBePredicted, TargetEntity, VisibilityMode,
    };
    use crate::shared::replication::components::{
        Controlled, Replicating, ReplicationGroupId, ReplicationGroupIdBuilder,
        ShouldBeInterpolated,
    };
    use crate::shared::replication::entity_map::{InterpolatedEntityMap, PredictedEntityMap};
    use crate::shared::replication::network_target::NetworkTarget;
    use bevy::prelude::{App, Plugin};

    pub(crate) struct SharedPlugin;

    impl Plugin for SharedPlugin {
        fn build(&self, app: &mut App) {
            // REFLECTION
            app.register_type::<TargetEntity>()
                .register_type::<Replicated>()
                .register_type::<Controlled>()
                .register_type::<Replicating>()
                .register_type::<ReplicationTarget>()
                .register_type::<ReplicateToServer>()
                .register_type::<ReplicateHierarchy>()
                .register_type::<ReplicationGroupIdBuilder>()
                .register_type::<ReplicationGroup>()
                .register_type::<ReplicationGroupId>()
                .register_type::<VisibilityMode>()
                .register_type::<NetworkTarget>()
                .register_type::<ShouldBeInterpolated>()
                .register_type::<PrePredicted>()
                .register_type::<ShouldBePredicted>()
                .register_type::<RemoteEntityMap>()
                .register_type::<PredictedEntityMap>()
                .register_type::<InterpolatedEntityMap>();
        }
    }
}
