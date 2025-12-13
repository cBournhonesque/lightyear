use crate::control::ControlledBy;
use crate::delta::DeltaManager;
use crate::hierarchy::ReplicateLike;
use crate::messages::actions::{ActionsChannel, ActionsMessageSend};
use crate::messages::metadata::{MetadataChannel, SenderMetadata};
use crate::messages::serialized_data::SerializedData;
use crate::messages::updates::UpdatesChannel;
use crate::plugin::ReplicationSystems;
use crate::prelude::{NetworkVisibility, ReplicationState};
use crate::prespawn;
use crate::registry::registry::ComponentRegistry;
use crate::send::buffer;
use crate::send::buffer::ReplicationMetadata;
use crate::send::client_pools::ClientPools;
use crate::send::components::{Replicate, Replicating, ReplicationGroup};
use crate::send::sender::ReplicationSender;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::entity::EntityIndexSet;
use bevy_ecs::prelude::*;
use bevy_ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick, SystemParamBuilder};
use bevy_time::{Real, Time};
use lightyear_connection::client::{Connected, Disconnected};
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_core::time::TickDelta;
use lightyear_link::prelude::{LinkOf, Server};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::EventSender;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::plugin::TransportSystems;
use lightyear_transport::prelude::Transport;
#[cfg(feature = "metrics")]
use lightyear_utils::metrics::DormantTimerGauge;
#[allow(unused_imports)]
use tracing::{error, info, warn};

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

    /// Sends pending [`Updates`] and [`Actions`] for each [`ReplicationSender`].
    fn send_messages(
        time: Res<Time>,
        metadata: Res<ReplicationMetadata>,
        mut serialized: ResMut<SerializedData>,
        mut pools: ResMut<ClientPools>,
        mut senders: Query<(Entity, &mut ReplicationSender, &mut Transport)>,
    ) -> Result<(), BevyError> {
        // TODO: get the local tick
        let tick = Tick(0);
        senders
            .iter_mut()
            .try_for_each(|(sender_entity, mut sender, mut transport)| {
                if !sender.pending_actions.is_empty() {
                    // TODO: should the tick be included in the message?
                    //  normally it's added by the Transport plugin, but what if the message is not sent
                    //  because of priority?
                    let actions = ActionsMessageSend::new(&sender.pending_actions, &serialized);
                    sender.sender_ticks.action_tick = tick;
                    actions.to_bytes(&mut sender.writer)?;
                    let message_bytes = sender.writer.split();
                    transport
                        .send_mut_with_priority::<ActionsChannel>(message_bytes, priority)?
                        .expect("The entity actions channels should always return a message_id");
                }

                // TODO: what is track_mutate_message?
                if !sender.pending_updates.is_empty() {
                    // let server_tick_range =
                    //     server_tick.write_cached(&mut serialized, &mut server_tick_range)?;

                    sender.pending_updates.send(
                        &mut sender.writer,
                        transport.as_mut(),
                        &mut sender.sender_ticks,
                        &mut pools,
                        &serialized,
                        tick,
                        metadata.change_tick,
                    )?;
                }
                Ok(())
            })?;

        Ok(())
    }

    /// Check which replication messages were actually sent, and update the
    /// priority accordingly
    fn update_priority(
        mut query: Query<(&mut ReplicationSender, &mut Transport), With<Connected>>,
    ) {
        query
            .par_iter_mut()
            .for_each(|(mut sender, mut transport)| {
                if !sender.send_timer.is_finished() {
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
            (Entity, &ReplicationSender, &mut EventSender<SenderMetadata>),
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
        mut replicate: Query<&mut ReplicationState>,
    ) {
        if let Ok(mut sender) = query.get_mut(trigger.entity) {
            *sender = ReplicationSender::new(
                sender.send_interval(),
                sender.send_updates_mode,
                sender.bandwidth_cap_enabled,
            );
        }
        replicate.iter_mut().for_each(|mut r| {
            r.per_sender_state.swap_remove(&trigger.entity);
        });
    }
}

impl Plugin for ReplicationSendPlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.init_resource::<ReplicableRootEntities>();

        // PLUGINS
        if !app.is_plugin_added::<crate::plugin::SharedPlugin>() {
            app.add_plugins(crate::plugin::SharedPlugin);
        }
        if !app.is_plugin_added::<prespawn::PreSpawnedPlugin>() {
            app.add_plugins(prespawn::PreSpawnedPlugin);
        }

        // SETS
        app.configure_sets(
            PostUpdate,
            (
                // buffer the messages before we send them
                (ReplicationSystems::Send, MessageSystems::Send).chain(),
                (
                    ReplicationBufferSystems::BeforeBuffer,
                    ReplicationBufferSystems::Buffer,
                    ReplicationBufferSystems::AfterBuffer,
                    ReplicationBufferSystems::Flush,
                )
                    .chain()
                    .in_set(ReplicationSystems::Send),
            ),
        );

        // SYSTEMS
        app.add_observer(buffer::buffer_entity_despawn_replicate_remove);
        app.add_observer(Self::send_sender_metadata);
        app.add_observer(Replicate::handle_connection);
        app.add_observer(Self::handle_disconnection);
        app.add_observer(ControlledBy::handle_disconnection);

        app.add_systems(
            PostUpdate,
            Self::handle_acks.in_set(ReplicationBufferSystems::BeforeBuffer),
        );
        app.add_systems(
            PostUpdate,
            Self::update_priority.after(TransportSystems::Send),
        );

        // TODO:
        // - change actions channel to sequenced reliable
        // - update priority before buffering messages
        // -
        app.add_systems(
            PostUpdate,
            Self::send_messages.in_set(ReplicationBufferSystems::Flush),
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

        let buffer_component_remove = (
            QueryParamBuilder::new(|builder| {
                // Or<(With<ReplicateLike>, (With<Replicating>, With<Replicate>))>
                builder.or(|b| {
                    b.with::<ReplicateLike>();
                    b.and(|b| {
                        b.with::<Replicating>();
                        b.with::<Replicate>();
                        b.with::<ReplicationState>();
                    });
                });
                builder.optional(|b| {
                    b.data::<(
                        &ReplicateLike,
                        &Replicate,
                        &ReplicationState,
                        &Replicating,
                        &NetworkVisibility,
                        &ReplicationGroup,
                    )>();
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
            buffer::replicate.in_set(ReplicationBufferSystems::Buffer),
        );

        app.world_mut().insert_resource(component_registry);
    }
}

#[deprecated(note = "Use ReplicationBufferSystems instead")]
pub type ReplicationBufferSet = ReplicationBufferSystems;

/// System sets to order systems that buffer updates that need to be replicated
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ReplicationBufferSystems {
    BeforeBuffer,
    // Buffer any replication updates in the ReplicationSender
    Buffer,
    AfterBuffer,
    // Flush the buffered replication messages to the Transport
    Flush,
}

/// Global list of root entities that should be considered for replication
///
/// Equivalent to Query<(), (With<Replicate>, With<Replicating>)> but we cache the result
#[derive(Resource, Default)]
pub(crate) struct ReplicableRootEntities {
    pub(crate) entities: EntityIndexSet,
}
