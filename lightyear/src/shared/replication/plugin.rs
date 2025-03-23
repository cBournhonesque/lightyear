//! This module contains the `ReplicationReceivePlugin` and `ReplicationSendPlugin` plugins, which control
//! the replication of entities and resources.
//!
use crate::shared::replication::hierarchy::{RelationshipReceivePlugin, RelationshipSendPlugin};
use crate::shared::replication::resources::{
    receive::ResourceReceivePlugin, send::ResourceSendPlugin,
};
use crate::shared::replication::systems;
use crate::shared::replication::{ReplicationReceive, ReplicationSend};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet, MainSet};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;

#[derive(Clone, Copy, Debug, Reflect)]
pub struct ReplicationConfig {
    /// How do send component updates?
    pub send_updates_mode: SendUpdatesMode,
}

#[derive(Clone, Copy, Debug, Reflect)]
pub enum SendUpdatesMode {
    /// We send all the updates that happened since the last tick when we received an ACK from the remote
    ///
    /// E.g. if the component was updated at tick 3; we will send the update at tick 3, and then at tick 4,
    /// we will send the update again even if the component wasn't updated, because we still haven't
    /// received an ACK from the client.
    SinceLastAck,
    // TODO: this is currently bugged because we need to maintain a `send_tick` / `ack_tick` per (entity, component)
    /// We send all the updates that happened since the last tick where we **sent** an update.
    /// E.g. if the component was updated at tick 3; we will send the update at tick 3, and then at tick 4,
    /// we won't be sending anything since the component wasn't updated after that.
    ///
    /// 99% of the time the packets don't get lost so this is fine to do, and allows us to save bandwidth
    /// by not sending the same update multiple time.
    ///
    /// If we receive a NACK (i.e. the packet got lost), we will send the updates since the last ACK.
    SinceLastSend,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            send_updates_mode: SendUpdatesMode::SinceLastAck,
        }
    }
}

pub(crate) mod receive {
    use super::*;
    pub(crate) struct ReplicationReceivePlugin<R> {
        clean_interval: Duration,
        _marker: core::marker::PhantomData<R>,
    }

    impl<R> ReplicationReceivePlugin<R> {
        pub(crate) fn new(tick_interval: Duration) -> Self {
            Self {
                // TODO: find a better constant for the clean interval?
                clean_interval: tick_interval * (i16::MAX as u32 / 3),
                _marker: core::marker::PhantomData,
            }
        }
    }

    impl<R: ReplicationReceive> Plugin for ReplicationReceivePlugin<R> {
        fn build(&self, app: &mut App) {
            // PLUGINS
            if !app.is_plugin_added::<shared::SharedPlugin>() {
                app.add_plugins(shared::SharedPlugin);
            }
            app.add_plugins(RelationshipReceivePlugin::<R, ChildOf>::default())
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
    use crate::shared::replication::hierarchy::HierarchySendPlugin;

    pub(crate) struct ReplicationSendPlugin<R> {
        send_interval: Duration,
        clean_interval: Duration,
        _marker: core::marker::PhantomData<R>,
    }

    #[derive(Resource, Debug)]
    pub(crate) struct SendIntervalTimer<R: Send + Sync + 'static> {
        pub(crate) timer: Option<Timer>,
        _marker: core::marker::PhantomData<R>,
    }

    impl<R: Send + Sync + 'static> ReplicationSendPlugin<R> {
        pub(crate) fn new(tick_interval: Duration, send_interval: Duration) -> Self {
            Self {
                send_interval,
                // TODO: find a better constant for the clean interval?
                clean_interval: tick_interval * (i16::MAX as u32 / 3),
                _marker: core::marker::PhantomData,
            }
        }

        /// Tick the timer that controls when we buffer replication updates
        fn tick_send_interval_timer(
            time_manager: Res<TimeManager>,
            mut timer: ResMut<SendIntervalTimer<R>>,
        ) {
            if let Some(timer) = &mut timer.timer {
                timer.tick(time_manager.delta());
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

            // we add this condition to not add a plugin twice if running in HostServer mode
            if !app.is_plugin_added::<shared::SharedPlugin>() {
                app.add_plugins(shared::SharedPlugin);
            }
            if !app.is_plugin_added::<RelationshipSendPlugin<ChildOf>>() {
                app.add_plugins(RelationshipSendPlugin::<ChildOf>::default());
            }
            app.add_plugins(ResourceSendPlugin::<R>::default());
            app.add_plugins(HierarchySendPlugin::<R>::default());

            // RESOURCES
            app.insert_resource(SendIntervalTimer::<R> {
                timer: if self.send_interval == Duration::default() {
                    None
                } else {
                    Some(Timer::new(self.send_interval, TimerMode::Repeating))
                },
                _marker: core::marker::PhantomData,
            });

            // SETS
            app.configure_sets(
                PostUpdate,
                (
                    // only send messages if the timer has finished
                    InternalReplicationSet::<R::SetMarker>::SendMessages.run_if(
                        |timer: Res<SendIntervalTimer<R>>| {
                            if let Some(timer) = &timer.timer {
                                timer.finished()
                            } else {
                                true
                            }
                        },
                    ),
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
                    )
                        .in_set(InternalReplicationSet::<R::SetMarker>::Buffer),
                    (
                        InternalReplicationSet::<R::SetMarker>::BufferEntityUpdates,
                        InternalReplicationSet::<R::SetMarker>::BufferResourceUpdates,
                        InternalReplicationSet::<R::SetMarker>::BufferComponentUpdates,
                        // TODO: verify this, why does handle-replicate-update need to run every frame?
                        //  because Removed<Replicate> is cleared every frame?
                        // NOTE: HandleReplicateUpdate should also run every frame?
                        InternalReplicationSet::<R::SetMarker>::AfterBuffer,
                    )
                        .in_set(InternalReplicationSet::<R::SetMarker>::SendMessages),
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
                        InternalMainSet::<R::SetMarker>::Send,
                    )
                        .chain(),
                ),
            );
            // SYSTEMS

            app.add_systems(
                PreUpdate,
                ReplicationSendPlugin::<R>::tick_send_interval_timer.after(MainSet::Receive),
            );
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
    use crate::client::components::ComponentSyncMode;
    use crate::client::replication::send::ReplicateToServer;
    use crate::prelude::{
        AppComponentExt, AppMessageExt, ChannelDirection, ComponentRegistry, NetworkRelevanceMode,
        PrePredicted, PreSpawned, RelationshipSync, RemoteEntityMap, Replicated, ReplicationConfig,
        ReplicationGroup, ShouldBePredicted, TargetEntity,
    };
    use crate::server::replication::send::ReplicateToClient;
    use crate::shared::replication::authority::{AuthorityChange, AuthorityPeer, HasAuthority};
    use crate::shared::replication::components::{
        Controlled, DisableReplicateHierarchy, Replicating, ReplicationGroupId,
        ReplicationGroupIdBuilder, ShouldBeInterpolated,
    };
    use crate::shared::replication::entity_map::{InterpolatedEntityMap, PredictedEntityMap};
    use crate::shared::replication::hierarchy::ReplicateLike;
    use crate::shared::replication::network_target::NetworkTarget;
    use bevy::prelude::{App, ChildOf, Plugin};

    pub(crate) struct SharedPlugin;

    impl Plugin for SharedPlugin {
        fn build(&self, app: &mut App) {
            // REFLECTION
            app.register_type::<TargetEntity>()
                .register_type::<Replicated>()
                .register_type::<Controlled>()
                .register_type::<Replicating>()
                .register_type::<ReplicateToClient>()
                .register_type::<ReplicateToServer>()
                .register_type::<DisableReplicateHierarchy>()
                .register_type::<ReplicateLike>()
                .register_type::<ReplicationGroupIdBuilder>()
                .register_type::<ReplicationGroup>()
                .register_type::<ReplicationConfig>()
                .register_type::<ReplicationGroupId>()
                .register_type::<NetworkRelevanceMode>()
                .register_type::<NetworkTarget>()
                .register_type::<ShouldBeInterpolated>()
                .register_type::<PrePredicted>()
                .register_type::<ShouldBePredicted>()
                .register_type::<RemoteEntityMap>()
                .register_type::<PredictedEntityMap>()
                .register_type::<HasAuthority>()
                .register_type::<AuthorityPeer>()
                .register_type::<InterpolatedEntityMap>();
        }

        fn finish(&self, app: &mut App) {
            // PROTOCOL
            // we register components here because
            // - we need to run this in `finish` so that all plugins have been built (so ClientPlugin and ServerPlugin
            // both exists)
            // - the replication::SharedPlugin should only be added once, even when running in host-server mode
            app.register_component::<PreSpawned>(ChannelDirection::Bidirectional);
            app.register_component::<PrePredicted>(ChannelDirection::Bidirectional);
            app.register_component::<ShouldBePredicted>(ChannelDirection::ServerToClient);
            app.register_component::<ShouldBeInterpolated>(ChannelDirection::ServerToClient);
            app.register_component::<RelationshipSync<ChildOf>>(ChannelDirection::Bidirectional)
                // to replicate ReplicationSync on the predicted/interpolated entities so that they spawn their own hierarchies
                .add_prediction(ComponentSyncMode::Simple)
                .add_interpolation(ComponentSyncMode::Simple)
                .add_map_entities();
            app.register_component::<Controlled>(ChannelDirection::ServerToClient)
                .add_prediction(ComponentSyncMode::Once)
                .add_interpolation(ComponentSyncMode::Once);

            app.register_message::<AuthorityChange>(ChannelDirection::ServerToClient)
                .add_map_entities();

            // check that the protocol was built correctly
            app.world().resource::<ComponentRegistry>().check();
        }
    }
}
