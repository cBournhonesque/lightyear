use crate::control::ControlledBy;
use crate::delta::DeltaManager;
use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};
use crate::message::{
    ActionsMessage, MetadataChannel, SenderMetadata, UpdatesChannel, UpdatesMessage,
};
use crate::plugin::ReplicationSet;
use crate::prelude::NetworkVisibility;
use crate::registry::registry::ComponentRegistry;
use crate::send::buffer;
#[cfg(feature = "interpolation")]
use crate::send::components::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::send::components::PredictionTarget;
use crate::send::components::{Replicate, Replicating, ReplicationGroup};
use crate::send::sender::ReplicationSender;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::{
    schedule::{IntoScheduleConfigs, SystemSet},
    system::{ParamBuilder, Query, QueryParamBuilder, Res, SystemChangeTick, SystemParamBuilder},
    world::Add,
};
use bevy_time::{Real, Time};
use lightyear_connection::client::{Connected, Disconnected};
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::TickDelta;
use lightyear_core::timeline::NetworkTimeline;
use lightyear_link::prelude::{LinkOf, Server};
use lightyear_messages::plugin::MessageSet;
use lightyear_messages::prelude::EventSender;
use lightyear_messages::registry::{MessageKind, MessageRegistry};
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::plugin::TransportSet;
use lightyear_transport::prelude::Transport;
use tracing::{error, warn};

pub struct ReplicationSendPlugin;

impl ReplicationSendPlugin {
    /// Before buffering messages, tick the timers and handle the acks
    fn handle_acks(
        time: Res<Time<Real>>,
        component_registry: Res<ComponentRegistry>,
        change_tick: SystemChangeTick,
        mut query: Query<
            (
                &mut ReplicationSender,
                &mut Transport,
                Option<&DeltaManager>,
                Option<&LinkOf>,
            ),
            With<Connected>,
        >,
        delta_query: Query<&DeltaManager, With<Server>>,
    ) {
        query
            .par_iter_mut()
            .for_each(|(mut sender, mut transport, delta, link_of)| {
                // TODO: maybe precompute for every entity DeltaManagerChildOf?
                // delta: either the delta manager is present on the sender directly (Client)
                // or the delta is on the server
                let delta = delta.or_else(|| link_of.and_then(|l| delta_query.get(l.server).ok()));

                let bevy_tick = change_tick.this_run();
                sender.send_timer.tick(time.delta());
                let update_nacks = &mut transport
                    .senders
                    .get_mut(&ChannelKind::of::<UpdatesChannel>())
                    .unwrap()
                    .message_nacks;
                sender.handle_nacks(bevy_tick, update_nacks);
                let update_acks = &mut transport
                    .senders
                    .get_mut(&ChannelKind::of::<UpdatesChannel>())
                    .unwrap()
                    .message_acks;
                // TODO: should we also handle ActionsChannel acks?
                sender.handle_acks(&component_registry, delta, update_acks);
            });
    }

    fn send_replication_messages(
        time: Res<Time<Real>>,
        message_registry: Res<MessageRegistry>,
        change_tick: SystemChangeTick,
        // We send messages directly through the transport instead of MessageSender<EntityActionsMessage>
        // but I don't remember why
        mut query: Query<(&mut ReplicationSender, &mut Transport, &LocalTimeline), With<Connected>>,
    ) {
        let actions_net_id = *message_registry
            .kind_map
            .net_id(&MessageKind::of::<ActionsMessage>())
            .unwrap();
        let updates_net_id = *message_registry
            .kind_map
            .net_id(&MessageKind::of::<UpdatesMessage>())
            .unwrap();
        query
            .par_iter_mut()
            .for_each(|(mut sender, mut transport, timeline)| {
                if !sender.send_timer.finished() {
                    return;
                }
                let bevy_tick = change_tick.this_run();
                sender.send_timer.reset();
                // TODO: also tick ReplicationGroups?
                sender.accumulate_priority(&time);
                sender
                    .send_actions_messages(
                        timeline.tick(),
                        bevy_tick,
                        &mut transport,
                        actions_net_id,
                    )
                    .inspect_err(|e| error!("Error buffering ActionsMessage: {e:?}"))
                    .ok();
                sender
                    .send_updates_messages(
                        timeline.tick(),
                        bevy_tick,
                        &mut transport,
                        updates_net_id,
                    )
                    .inspect_err(|e| error!("Error buffering UpdatesMessage: {e:?}"))
                    .ok();
            });
    }

    /// Check which replication messages were actually sent, and update the
    /// priority accordingly
    fn update_priority(
        mut query: Query<(&mut ReplicationSender, &mut Transport), With<Connected>>,
    ) {
        query
            .par_iter_mut()
            .for_each(|(mut sender, mut transport)| {
                if !sender.send_timer.finished() {
                    return;
                }
                let messages_sent = &mut transport
                    .senders
                    .get_mut(&ChannelKind::of::<UpdatesChannel>())
                    .unwrap()
                    .messages_sent;
                sender.recv_send_notification(messages_sent);
            });
    }

    /// Send a message containing metadata about the sender
    fn send_sender_metadata(
        // NOTE: it's important to trigger on both Add<Connected> and Add<ReplicationSender> because the ClientOf could be
        //  added BEFORE the ReplicationSender is added. (ClientOf is spawned by netcode, ReplicationSender is added by the user)
        trigger: On<Add, (Connected, ReplicationSender)>,
        tick_duration: Res<TickDuration>,
        mut query: Query<
            (
                Entity,
                &ReplicationSender,
                &mut EventSender<SenderMetadata>,
            ),
            With<Connected>,
        >,
    ) {
        if let Ok((sender_entity, sender, mut trigger_sender)) = query.get_mut(trigger.entity) {
            let send_interval = sender.send_interval();
            let send_interval_delta = TickDelta::from_duration(send_interval, tick_duration.0);
            let metadata = SenderMetadata {
                send_interval: send_interval_delta.into(),
                sender_entity,
            };
            trigger_sender.trigger::<MetadataChannel>(metadata);
        }
    }

    /// On disconnect, reset the replication sender to its original state
    fn handle_disconnection(
        trigger: On<Add, Disconnected>,
        mut query: Query<&mut ReplicationSender>,
    ) {
        if let Ok(mut sender) = query.get_mut(trigger.entity) {
            *sender = ReplicationSender::new(
                sender.send_interval(),
                sender.send_updates_mode,
                sender.bandwidth_cap_enabled,
            );
        }
    }

    // /// Tick the internal timers of all replication groups.
    // fn tick_replication_group_timers(
    //     time_manager: Res<TimeManager>,
    //     mut replication_groups: Query<&mut ReplicationGroup, With<Replicating>>,
    // ) {
    //     for mut replication_group in replication_groups.iter_mut() {
    //         if let Some(send_frequency) = &mut replication_group.send_frequency {
    //             send_frequency.tick(time_manager.delta());
    //             if send_frequency.finished() {
    //                 replication_group.should_send = true;
    //             }
    //         }
    //     }
    // }

    // /// After we buffer updates, reset all the `should_send` to false
    // /// for the replication groups that have a `send_frequency`
    // fn update_replication_group_should_send(
    //     mut replication_groups: Query<&mut ReplicationGroup, With<Replicating>>,
    // ) {
    //     for mut replication_group in replication_groups.iter_mut() {
    //         if replication_group.send_frequency.is_some() {
    //             replication_group.should_send = false;
    //         }
    //     }
    // }
}

impl Plugin for ReplicationSendPlugin {
    fn build(&self, app: &mut App) {
        // PLUGINS
        if !app.is_plugin_added::<crate::plugin::SharedPlugin>() {
            app.add_plugins(crate::plugin::SharedPlugin);
        }

        // SETS
        app.configure_sets(
            PostUpdate,
            (
                // buffer the messages before we send them
                (ReplicationSet::Send, MessageSet::Send).chain(),
                (
                    ReplicationBufferSet::BeforeBuffer,
                    ReplicationBufferSet::Buffer,
                    ReplicationBufferSet::AfterBuffer,
                    ReplicationBufferSet::Flush,
                )
                    .chain()
                    .in_set(ReplicationSet::Send),
            ),
        );

        // SYSTEMS
        app.add_observer(buffer::buffer_entity_despawn_replicate_remove);
        app.add_observer(Self::send_sender_metadata);
        app.add_observer(Replicate::handle_connection);
        #[cfg(feature = "prediction")]
        {
            app.add_observer(PredictionTarget::handle_connection);
            app.add_observer(PredictionTarget::add_replication_group);
        }
        #[cfg(feature = "interpolation")]
        app.add_observer(InterpolationTarget::handle_connection);
        app.add_observer(Self::handle_disconnection);

        app.add_observer(ControlledBy::handle_disconnection);

        app.add_systems(
            PostUpdate,
            Self::handle_acks.in_set(ReplicationBufferSet::BeforeBuffer),
        );
        app.add_systems(
            PostUpdate,
            buffer::buffer_entity_despawn_replicate_updated.in_set(ReplicationBufferSet::Buffer),
        );
        app.add_systems(
            PostUpdate,
            buffer::update_cached_replicate_post_buffer.in_set(ReplicationBufferSet::AfterBuffer),
        );
        app.add_systems(PostUpdate, Self::update_priority.after(TransportSet::Send));
        app.add_systems(
            PostUpdate,
            Self::send_replication_messages.in_set(ReplicationBufferSet::Flush),
        );

        // app.add_systems(
        //     PostUpdate,
        //     (
        //         crate::send_plugin::ReplicationSendPlugin::tick_replication_group_timers
        //             .in_set(InternalReplicationSet::<R::SetMarker>::BeforeBuffer),
        //         crate::send_plugin::ReplicationSendPlugin::update_replication_group_should_send
        //             // note that this runs every send_interval
        //             .in_set(InternalReplicationSet::<R::SetMarker>::AfterBuffer),
        //     ),
        // );
    }

    fn finish(&self, app: &mut App) {
        if !app.world().contains_resource::<ComponentRegistry>() {
            warn!("ReplicationSendPlugin: ComponentRegistry not found, adding it");
            app.world_mut().init_resource::<ComponentRegistry>();
        }
        // temporarily remove component_registry from the app to enable split borrows
        let component_registry = app
            .world_mut()
            .remove_resource::<ComponentRegistry>()
            .unwrap();

        let replicate = (
            QueryParamBuilder::new(|builder| {
                // Or<(With<ReplicateLike>, (With<Replicating>, With<Replicate>))>
                builder.or(|b| {
                    b.with::<ReplicateLikeChildren>();
                    b.with::<ReplicateLike>();
                    b.and(|b| {
                        b.with::<Replicating>();
                        b.with::<Replicate>();
                    });
                });
                builder.optional(|b| {
                    b.data::<(
                        &Replicate,
                        &ReplicationGroup,
                        &NetworkVisibility,
                        &ReplicateLikeChildren,
                        &ReplicateLike,
                        &ControlledBy,
                    )>();
                    #[cfg(feature = "prediction")]
                    b.data::<&PredictionTarget>();
                    #[cfg(feature = "interpolation")]
                    b.data::<&InterpolationTarget>();
                    // include access to &C and &ComponentReplicationOverrides<C> for all replication components with the right direction
                    component_registry
                        .component_metadata_map
                        .iter()
                        .for_each(|(kind, m)| {
                            let id = m.component_id;
                            b.ref_id(id);
                            if let Some(r) = &m.replication {
                                b.ref_id(r.overrides_component_id);
                            }
                        });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(buffer::replicate)
            .with_name("ReplicationSendPlugin::replicate");
        // let replicate = (
        //     QueryParamBuilder::new(|builder| {
        //         // Or<(With<ReplicateLike>, With<ReplicateLikeChildren>, (With<Replicating>, With<Replicate>))>
        //         builder.or(|b| {
        //             b.with::<ReplicateLikeChildren>();
        //             b.with::<ReplicateLike>();
        //             b.and(|b| {
        //                 b.with::<Replicating>();
        //                 b.with::<Replicate>();
        //             });
        //         });
        //         builder.optional(|b| {
        //             b.data::<(
        //                 &Replicate,
        //                 &ReplicationGroup,
        //                 &NetworkVisibility,
        //                 &ReplicateLikeChildren,
        //                 &ReplicateLike,
        //                 &ControlledBy,
        //             )>();
        //             #[cfg(feature = "prediction")]
        //             b.data::<&PredictionTarget>();
        //             #[cfg(feature = "interpolation")]
        //             b.data::<&InterpolationTarget>();
        //             // include access to &C and &ComponentReplicationOverrides<C> for all replication components with the right direction
        //             component_registry
        //                 .replication_map
        //                 .iter()
        //                 .for_each(|(kind, _)| {
        //                     let id = component_registry.kind_to_component_id.get(kind).unwrap();
        //                     b.ref_id(*id);
        //                     let override_id = component_registry
        //                         .replication_map
        //                         .get(kind)
        //                         .unwrap()
        //                         .overrides_component_id;
        //                     b.ref_id(override_id);
        //                 });
        //         });
        //     }),
        //     ParamBuilder,
        //     ParamBuilder,
        //     ParamBuilder,
        //     ParamBuilder,
        //     ParamBuilder,
        //     ParamBuilder,
        //     ParamBuilder,
        //     ParamBuilder,
        // )
        //     .build_state(app.world_mut())
        //     .build_system(buffer::replicate_bis)
        //     .with_name("ReplicationSendPlugin::replicate_bis");

        let buffer_component_remove = (
            QueryParamBuilder::new(|builder| {
                // Or<(With<ReplicateLike>, (With<Replicating>, With<Replicate>))>
                builder.or(|b| {
                    b.with::<ReplicateLike>();
                    b.and(|b| {
                        b.with::<Replicating>();
                        b.with::<Replicate>();
                    });
                });
                builder.optional(|b| {
                    b.data::<(&ReplicateLike, &Replicate, &ReplicationGroup)>();
                    // include access to &C and &ComponentReplicationOverrides<C> for all replication components with the right direction
                    component_registry
                        .component_metadata_map
                        .iter()
                        .for_each(|(kind, m)| {
                            b.ref_id(m.component_id);
                            if let Some(r) = &m.replication {
                                b.ref_id(r.overrides_component_id);
                            }
                        });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system_with_input(buffer::buffer_component_removed)
            .with_name("ReplicationSendPlugin::buffer_component_removed");

        let mut buffer_component_remove_observer = Observer::new(buffer_component_remove);
        for component in component_registry.component_id_to_kind.keys() {
            buffer_component_remove_observer =
                buffer_component_remove_observer.with_component(*component);
        }
        app.world_mut().spawn(buffer_component_remove_observer);

        app.add_systems(
            PostUpdate,
            // TODO: putting it here means we might miss entities that are spawned and despawned within the send_interval? bug or feature?
            replicate.in_set(ReplicationBufferSet::Buffer),
        );

        app.world_mut().insert_resource(component_registry);
    }
}

/// System sets to order systems that buffer updates that need to be replicated
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ReplicationBufferSet {
    BeforeBuffer,
    // Buffer any replication updates in the ReplicationSender
    Buffer,
    AfterBuffer,
    // Flush the buffered replication messages to the Transport
    Flush,
}
