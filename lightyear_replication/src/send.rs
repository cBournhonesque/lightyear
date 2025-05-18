//! General struct handling replication
use core::iter::Extend;

use super::message::{
    ActionsChannel, EntityActions, MetadataChannel, SendEntityActionsMessage, SenderMetadata,
    SpawnAction, UpdatesChannel, UpdatesSendMessage,
};
use super::message::{ActionsMessage, UpdatesMessage};
use crate::buffer;
use crate::buffer::Replicate;
#[cfg(feature = "interpolation")]
use crate::components::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::components::PredictionTarget;
use crate::components::{Replicating, ReplicationGroup, ReplicationGroupId};
use crate::control::ControlledBy;
use crate::delta::DeltaManager;
use crate::error::ReplicationError;
use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};
use crate::plugin::ReplicationSet;
use crate::prelude::NetworkVisibility;
use crate::registry::registry::ComponentRegistry;
use crate::registry::{ComponentError, ComponentKind, ComponentNetId};
#[cfg(not(feature = "std"))]
use alloc::{string::ToString, vec::Vec};
use bevy::app::{App, Plugin, PostUpdate};
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::{EntityHash, EntityIndexMap};
use bevy::ecs::system::{ParamBuilder, QueryParamBuilder, SystemChangeTick};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::ptr::Ptr;
use bytes::Bytes;
use core::time::Duration;
use lightyear_connection::client::{Connected, Disconnected};
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_core::time::TickDelta;
use lightyear_core::timeline::NetworkTimeline;
use lightyear_messages::plugin::MessageSet;
use lightyear_messages::prelude::TriggerSender;
use lightyear_messages::registry::{MessageKind, MessageRegistry};
use lightyear_messages::MessageNetId;
use lightyear_serde::entity_map::{RemoteEntityMap, SendEntityMap};
use lightyear_serde::writer::Writer;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::packet::message::MessageId;
use lightyear_transport::plugin::TransportSet;
use lightyear_transport::prelude::Transport;
use tracing::{debug, error, trace};
#[cfg(feature = "trace")]
use tracing::{instrument, Level};

type EntityHashMap<K, V> = HashMap<K, V, EntityHash>;
type EntityHashSet<K> = bevy::platform::collections::HashSet<K, EntityHash>;

/// When a [`EntityUpdatesMessage`](super::EntityUpdatesMessage) message gets buffered (and we have access to its [`MessageId`]),
/// we keep track of some information related to this message.
/// It is useful when we get notified that the message was acked or lost.
#[derive(Debug, PartialEq)]
pub(crate) struct UpdateMessageMetadata {
    /// The group id that this message is about
    group_id: ReplicationGroupId,
    /// The BevyTick at which we buffered the message
    bevy_tick: BevyTick,
    /// The tick at which we buffered the message
    tick: Tick,
    /// The (entity, component) pairs that were included in the message
    delta: Vec<(Entity, ComponentKind)>,
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

pub struct ReplicationSendPlugin;

#[derive(Resource, Debug)]
pub(crate) struct SendIntervalTimer {
    pub(crate) timer: Option<Timer>,
}

impl ReplicationSendPlugin {
    /// Before buffering messages, tick the timers and handle the acks
    fn handle_acks(
        time: Res<Time<Real>>,
        component_registry: Res<ComponentRegistry>,
        change_tick: SystemChangeTick,
        mut query: Query<
            (
                &mut ReplicationSender,
                &mut DeltaManager,
                &mut Transport,
            ),
            With<Connected>,
        >,
    ) {
        query
            .par_iter_mut()
            .for_each(|(mut sender, mut delta, mut transport)| {
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
                sender.handle_acks(&component_registry, &mut delta, update_acks);
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
        // NOTE: it's important to trigger on both OnAdd<Connected> and OnAdd<ReplicationSender> because the ClientOf could be
        //  added BEFORE the ReplicationSender is added. (ClientOf is spawned by netcode, ReplicationSender is added by the user)
        trigger: Trigger<OnAdd, (Connected, ReplicationSender)>,
        tick_duration: Res<TickDuration>,
        mut query: Query<(&ReplicationSender, &mut TriggerSender<SenderMetadata>), With<Connected>>,
    ) {
        if let Ok((sender, mut trigger_sender)) = query.get_mut(trigger.target()) {
            let send_interval = sender.send_interval();
            let send_interval_delta = TickDelta::from_duration(send_interval, tick_duration.0);
            let metadata = SenderMetadata {
                send_interval: send_interval_delta.into(),
            };
            trigger_sender.trigger::<MetadataChannel>(metadata);
        }
    }

    /// On disconnect, reset the replication sender to its original state
    fn handle_disconnection(
        trigger: Trigger<OnAdd, Disconnected>,
        mut query: Query<&mut ReplicationSender>,
    ) {
        if let Ok(mut sender) = query.get_mut(trigger.target()) {
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
                        .replication_map
                        .iter()
                        .for_each(|(kind, _)| {
                            let id = component_registry.kind_to_component_id.get(kind).unwrap();
                            b.ref_id(*id);
                            let override_id = component_registry
                                .replication_map
                                .get(kind)
                                .unwrap()
                                .overrides_component_id;
                            b.ref_id(override_id);
                        });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(buffer::replicate);

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
                        .replication_map
                        .iter()
                        .for_each(|(kind, _)| {
                            let id = component_registry.kind_to_component_id.get(kind).unwrap();
                            b.ref_id(*id);
                            let override_id = component_registry
                                .replication_map
                                .get(kind)
                                .unwrap()
                                .overrides_component_id;
                            b.ref_id(override_id);
                        });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system_with_input(buffer::buffer_component_removed);

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

#[derive(Component, Debug)]
#[require(Transport)]
#[require(LocalTimeline)]
#[require(DeltaManager)]
pub struct ReplicationSender {
    pub replicated_entities: EntityIndexMap<bool>,
    pub(crate) writer: Writer,
    /// Map from message-id to the corresponding group-id that sent this update message, as well as the `send_tick` BevyTick
    /// when we buffered the message. (so that when it's acked, we know we only need to include updates that happened after that tick,
    /// for that replication group)
    pub(crate) updates_message_id_to_group_id: HashMap<MessageId, UpdateMessageMetadata>,
    /// Group channels that have at least 1 replication update or action buffered
    pub group_with_actions: EntityHashSet<ReplicationGroupId>,
    pub group_with_updates: EntityHashSet<ReplicationGroupId>,
    /// Buffer to so that we have an ordered receiver per group
    pub group_channels: EntityHashMap<ReplicationGroupId, GroupChannel>,
    pub send_timer: Timer,
    /// ChangeTicks when we last sent replication messages for this Sender.
    /// We will compare this to component change ticks to determine if the change should be included.
    /// We cannot simply use the SystemTicks because the system runs every frame.
    pub(crate) this_run: BevyTick,
    pub(crate) last_run: BevyTick,
    /// Tick when we last did a cleanup
    pub(crate) last_cleanup_tick: Option<Tick>,
    send_updates_mode: SendUpdatesMode,
    // TODO: detect automatically if priority manager is enabled!
    bandwidth_cap_enabled: bool,
}

impl Default for ReplicationSender {
    fn default() -> Self {
        Self::new(Duration::default(), SendUpdatesMode::SinceLastAck, false)
    }
}

impl ReplicationSender {
    pub fn new(
        send_interval: Duration,
        send_updates_mode: SendUpdatesMode,
        bandwidth_cap_enabled: bool,
    ) -> Self {
        // make sure that the timer is finished when we start, to immediately start replicating
        let mut send_timer = Timer::new(send_interval, TimerMode::Repeating);
        send_timer.tick(Duration::MAX);
        Self {
            // SEND
            replicated_entities: EntityIndexMap::default(),
            writer: Writer::default(),
            updates_message_id_to_group_id: Default::default(),
            group_with_actions: EntityHashSet::default(),
            group_with_updates: EntityHashSet::default(),
            // pending_unique_components: EntityHashMap::default(),
            group_channels: Default::default(),
            send_updates_mode,
            // PRIORITY
            send_timer,
            this_run: BevyTick::MAX,
            last_run: BevyTick::MAX,
            last_cleanup_tick: None,
            bandwidth_cap_enabled,
        }
    }

    /// Returns true if the `Tick` was updated since the last time the Sender was buffering replication updates
    #[inline(always)]
    pub(crate) fn is_updated(&self, tick: BevyTick) -> bool {
        self.this_run == self.last_run || tick.is_newer_than(self.last_run, self.this_run)
    }

    pub fn send_interval(&self) -> Duration {
        self.send_timer.duration()
    }

    pub(crate) fn add_replicated_entity(&mut self, entity: Entity, authority: bool) {
        self.replicated_entities.insert(entity, authority);
    }

    pub fn gain_authority(&mut self, entity: Entity) {
        self.replicated_entities.insert(entity, true);
    }

    pub fn lose_authority(&mut self, entity: Entity) {
        self.replicated_entities.insert(entity, false);
    }

    /// Returns true if this sender has authority over the entity
    pub fn has_authority(&self, entity: Entity) -> bool {
        self.replicated_entities.get(&entity).is_some_and(|a| *a)
    }

    /// Get the `send_tick` for a given group.
    ///
    /// This is a bevy `Tick` and is used for change-detection.
    /// We will send all updates that happened after this bevy tick.
    pub(crate) fn get_send_tick(&self, group_id: ReplicationGroupId) -> Option<BevyTick> {
        self.group_channels
            .get(&group_id)
            .and_then(|channel| match self.send_updates_mode {
                SendUpdatesMode::SinceLastSend => channel.send_tick,
                SendUpdatesMode::SinceLastAck => channel.ack_bevy_tick,
            })
    }

    /// Internal bookkeeping:
    /// 1. handle all nack update messages (by resetting the send_tick to the previous ack_tick)
    pub(crate) fn handle_nacks(&mut self, world_tick: BevyTick, update_nacks: &mut Vec<MessageId>) {
        // 1. handle all nack update messages
        update_nacks.drain(..).for_each(|message_id| {
            // remember to remove the entry from the map to avoid memory leakage
            match self.updates_message_id_to_group_id.remove(&message_id)
            { Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                ..
            }) => {
                if let SendUpdatesMode::SinceLastSend = self.send_updates_mode {
                    match self.group_channels.get_mut(&group_id) { Some(channel) => {
                        // when we know an update message has been lost, we need to reset our send_tick
                        // to our previous ack_tick
                        trace!(
                            "Update channel send_tick back to ack_tick because a message has been lost"
                        );
                        // only reset the send tick if the bevy_tick of the message that was lost is
                        // newer than the current ack_tick
                        // (otherwise it just means we lost some old message, and we don't need to do anything)
                        if channel
                            .ack_bevy_tick
                            .is_some_and(|ack_tick| bevy_tick.is_newer_than(ack_tick, world_tick))
                        {
                            channel.send_tick = channel.ack_bevy_tick;
                        }

                        // TODO: if all clients lost a given message, than we can immediately drop the
                        //  delta-compression data for that tick
                    } _ => {
                        error!("Received an update message-id nack but the corresponding group channel does not exist");
                    }}
                }
            } _ => {
                // NOTE: this happens when a message-id is split between multiple packets (fragmented messages)
                trace!("Received an update message-id nack ({message_id:?}) but we don't know the corresponding group id");
            }}
        })
    }

    /// If we got notified that an update got send (included in a packet):
    /// - we reset the accumulated priority to 0.0 for all replication groups included in the message
    /// - we update the replication groups' send_tick
    ///   Then we accumulate the priority for all replication groups.
    ///
    /// This should be call after the Send SystemSet.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn recv_send_notification(&mut self, messages_sent: &mut Vec<MessageId>) {
        if !self.bandwidth_cap_enabled {
            return;
        }
        messages_sent.drain(..).for_each(|message_id| {
            match self.updates_message_id_to_group_id.get(&message_id)
            { Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                ..
            }) => {
                match self.group_channels.get_mut(group_id) { Some(channel) => {
                    // TODO: should we also reset the priority for replication-action messages?
                    // reset the priority
                    debug!(
                        ?message_id,
                        ?group_id,
                        "successfully sent message for replication group! Updating send_tick"
                    );
                    channel.send_tick = Some(*bevy_tick);
                    channel.accumulated_priority = 0.0;
                } _ => {
                    error!(?message_id, ?group_id, "Received a send message-id notification but the corresponding group channel does not exist");
                }}
            } _ => {
                error!(?message_id,
                    "Received an send message-id notification but we don't know the corresponding group id"
                );
            }}
        })
    }

    /// Handle a notification that a message got acked:
    /// - update the channel's ack_tick and ack_bevy_tick
    ///
    /// We call this after the Receive SystemSet; to update the bevy_tick at which we received entity updates for each group
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn handle_acks(
        &mut self,
        component_registry: &ComponentRegistry,
        delta_manager: &mut DeltaManager,
        update_acks: &mut Vec<MessageId>,
    ) {
        update_acks.drain(..).for_each(|message_id| {
            // remember to remove the entry from the map to avoid memory leakage
            match self.updates_message_id_to_group_id.remove(&message_id)
            { Some(UpdateMessageMetadata {
                group_id,
                bevy_tick,
                tick,
                delta,
            }) => {
                match self.group_channels.get_mut(&group_id) { Some(channel) => {
                    // update the ack tick for the channel
                    debug!(?group_id, ?bevy_tick, ?tick, "Update channel ack_tick");
                    channel.ack_bevy_tick = Some(bevy_tick);
                    // `delta_ack_ticks` won't grow indefinitely thanks to the cleanup systems
                    for (entity, component_kind) in delta {
                        channel
                            .delta_ack_ticks
                            .insert((entity, component_kind), tick);
                    }

                    // update the acks for the delta manager
                    delta_manager.receive_ack(tick, group_id, component_registry);
                } _ => {
                    error!("Received an update message-id ack but the corresponding group channel does not exist");
                }}
            } _ => {
                error!("Received an update message-id ack but we don't know the corresponding group id");
            }}
        })
    }

    /// Do some internal bookkeeping:
    /// - handle tick wrapping
    pub(crate) fn tick_cleanup(&mut self, tick: Tick) {
        // skip cleanup if we did one recently
        if self
            .last_cleanup_tick
            .is_some_and(|last| tick < last + (i16::MAX / 3))
        {
            return;
        }
        self.last_cleanup_tick = Some(tick);
        let delta = i16::MAX / 2;
        // if it's been enough time since we last any action for the group, we can set the last_action_tick to None
        // (meaning that there's no need when we receive the update to check if we have already received a previous action)
        for group_channel in self.group_channels.values_mut() {
            if let Some(last_action_tick) = group_channel.last_action_tick {
                if tick - last_action_tick > delta {
                    debug!(
                        ?tick,
                        ?last_action_tick,
                        ?group_channel,
                        "Setting the last_action tick to None because there hasn't been any new actions in a while"
                    );
                    group_channel.last_action_tick = None;
                }
            }
            group_channel
                .delta_ack_ticks
                .retain(|_, ack_tick| tick - *ack_tick <= delta);
        }
    }
}

/// We want:
/// - entity actions to be done reliably
/// - entity updates (component updates) to be done unreliably
///
/// - all component inserts/removes/updates for an entity to be grouped together in a single message
impl ReplicationSender {
    // TODO: how can I emit metrics here that contain the channel kind?
    //  use a OnceCell that gets set with the channel name mapping when the protocol is finalized?
    //  the other option is to have wrappers in Connection, but that's pretty ugly

    /// Host has spawned an entity, and we want to replicate this to remote
    /// Returns true if we should send a message
    // #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn prepare_entity_spawn(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        priority: f32,
    ) {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::send::entity_spawn").increment(1);
        }
        self.group_with_actions.insert(group_id);
        self.group_channels
            .entry(group_id)
            .or_default()
            .pending_actions
            .entry(entity)
            .or_default()
            .spawn = SpawnAction::Spawn;
        self.group_channels
            .entry(group_id)
            .or_default()
            .base_priority = priority;
    }

    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn prepare_entity_despawn(&mut self, entity: Entity, group_id: ReplicationGroupId) {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::send::entity_despawn").increment(1);
        }
        self.group_with_actions.insert(group_id);
        self.group_channels
            .entry(group_id)
            .or_default()
            .pending_actions
            .entry(entity)
            .or_default()
            .spawn = SpawnAction::Despawn;
    }

    /// Helper function to prepare component insert for components for which we know the type
    ///
    /// Only use this for components where we don't need EntityMapping
    pub(crate) fn prepare_typed_component_insert<C: Component>(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        component_registry: &ComponentRegistry,
        data: &C,
    ) -> Result<(), ComponentError> {
        component_registry.serialize(data, &mut self.writer, &mut SendEntityMap::default())?;
        let raw_data = self.writer.split();
        self.prepare_component_insert(entity, group_id, raw_data);
        Ok(())
    }

    // we want to send all component inserts that happen together for the same entity in a single message
    // (because otherwise the inserts might be received at different packets/ticks by the remote, and
    // the remote might expect the components insert to be received at the same time)
    // #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn prepare_component_insert(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        component: Bytes,
    ) {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::send::component_insert").increment(1);
        }
        self.group_with_actions.insert(group_id);
        self.group_channels
            .entry(group_id)
            .or_default()
            .pending_actions
            .entry(entity)
            .or_default()
            .insert
            .push(component);
    }

    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn prepare_component_remove(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        kind: ComponentNetId,
    ) {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::send::component_remove").increment(1);
        }
        self.group_with_actions.insert(group_id);
        self.group_channels
            .entry(group_id)
            .or_default()
            .pending_actions
            .entry(entity)
            .or_default()
            .remove
            .push(kind);
    }

    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn prepare_component_update(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        raw_data: Bytes,
    ) {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::send::component_update").increment(1);
        }
        self.group_with_updates.insert(group_id);
        self.group_channels
            .entry(group_id)
            .or_default()
            .pending_updates
            .entry(entity)
            .or_default()
            .push(raw_data);
    }

    /// Create a component update for a component that has delta compression enabled
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_delta_component_update(
        &mut self,
        entity: Entity,
        group_id: ReplicationGroupId,
        kind: ComponentKind,
        component_data: Ptr,
        registry: &ComponentRegistry,
        delta_manager: &mut DeltaManager,
        _tick: Tick,
        remote_entity_map: &mut RemoteEntityMap,
    ) -> Result<(), ReplicationError> {
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("replication::send::component_update_delta").increment(1);
        }
        let group_channel = self.group_channels.entry(group_id).or_default();
        // Get the latest acked tick for this entity/component
        let raw_data = group_channel
            .delta_ack_ticks
            .get(&(entity, kind))
            .map(|&ack_tick| {
                // we have an ack tick for this replication group, get the corresponding component value
                // so we can compute a diff
                let old_data = delta_manager
                    .data
                    // NOTE: remember to use the local entity for local bookkeeping
                    .get_component_value(entity, ack_tick, kind, group_id)
                    .ok_or(ReplicationError::DeltaCompressionError(
                        "could not find old component value to compute delta".to_string(),
                    ))
                    .inspect_err(|e| {
                        error!(
                            ?entity,
                            name = ?registry.name(kind),
                            "Could not find old component value from tick {:?} to compute delta: {e:?}",
                            ack_tick,
                        );
                        error!("DeltaManager data: {:?}", delta_manager.data);
                    })?;
                // SAFETY: the component_data and erased_data is a pointer to a component that corresponds to kind
                unsafe {
                    registry.serialize_diff(
                        ack_tick,
                        old_data,
                        component_data,
                        &mut self.writer,
                        kind,
                        &mut remote_entity_map.local_to_remote,
                    )?;
                }
                Ok::<Bytes, ReplicationError>(self.writer.split())
            })
            .unwrap_or_else(|| {
                // SAFETY: the component_data is a pointer to a component that corresponds to kind
                unsafe {
                    // compute a diff from the base value, and serialize that
                    registry.serialize_diff_from_base_value(
                        component_data,
                        &mut self.writer,
                        kind,
                        &mut remote_entity_map.local_to_remote,
                    )?;
                }
                Ok::<Bytes, ReplicationError>(self.writer.split())
            })?;
        trace!(?kind, "Inserting pending update!");
        // use the network entity when serializing
        let entity = remote_entity_map.to_remote(entity);
        self.prepare_component_update(entity, group_id, raw_data);
        self.group_channels
            .entry(group_id)
            .or_default()
            .pending_delta_updates
            .push((entity, kind));
        Ok(())
    }

    // TODO: the priority for entity actions should remain the base_priority,
    //  because the priority will get accumulated in the reliable channel
    //  For entity updates, we might want to use the multiplier, but not sure
    //  Maybe we just want to run the accumulate priority system every frame.
    /// Before sending replication messages, we accumulate the priority for all replication groups.
    ///
    /// (the priority starts at 0.0, and is accumulated for each group based on the base priority of the group)
    pub(crate) fn accumulate_priority(&mut self, time: &Time<Real>) {
        // let priority_multiplier = if self.replication_config.send_interval == Duration::default() {
        //     1.0
        // } else {
        //     (self.replication_config.send_interval.as_nanos() as f32
        //         / time_manager.delta().as_nanos() as f32)
        // };
        let priority_multiplier = 1.0;
        self.group_channels.values_mut().for_each(|channel| {
            trace!(
                "in accumulate priority: accumulated={:?} base={:?} multiplier={:?}, time_manager_delta={:?}",
                channel.accumulated_priority, channel.base_priority, priority_multiplier,
                time.delta().as_nanos()
            );
            channel.accumulated_priority += channel.base_priority * priority_multiplier;
        });
    }

    /// Prepare the [`EntityActionsMessage`](super::EntityActionsMessage) messages to send.
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn send_actions_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
        sender: &mut Transport,
        actions_net_id: MessageNetId,
    ) -> Result<(), ReplicationError> {
        self.group_with_actions.drain().try_for_each(|group_id| {
            // SAFETY: we know that the group_channel exists since group_with_actions contains the group_id
            let channel = self.group_channels.get_mut(&group_id).unwrap();
            let mut actions = core::mem::take(&mut channel.pending_actions);

            // TODO: should we be careful about not mapping entities for actions if it's a Spawn action?
            //  how could that happen?
            // Add any updates for that group
            if self.group_with_updates.remove(&group_id) {
                // drain so that we keep the allocated memory
                for (entity, components) in channel.pending_updates.drain() {
                    actions
                        .entry(entity)
                        .or_default()
                        .updates
                        .extend(components);
                }
                //  We can consider that we received an ack for the current tick because the message is sent reliably,
                //  so we know that we should eventually receive an ack.
                //  Updates after this insert only get read if the insert was received, so this doesn't introduce any bad behaviour.
                //  - For delta-compression: this is useful to compute future diffs from this Insert value immediately
                //  - in general: this is useful to avoid sending too many unnecessary updates. For example:
                //      - tick 3: C1 update
                //      - tick 4: C2 insert. C1 update. (if we send all updates since last_ack) !!!! We need to update the ack from the Insert only AFTER all the Updates are prepared!!!
                //      - tick 5: Before, we would send C1 update again, since we didn't receive an ack for C1 yet. But now we stop sending it because we know that the message from tick 4 will be received.
                for (entity, component_kind) in channel.pending_delta_updates.drain(..) {
                    channel
                        .delta_ack_ticks
                        .insert((entity, component_kind), tick);
                }
            }

            // update the send tick so that we don't send updates immediately after an insert message.
            // (which would happen because the send_tick is only set to Some(x) after an Update message is sent, so
            // when an entity is first spawned the send_tick is still None)
            // This is ok to do even if we don't get an actual send notification because EntityActions messages are
            // guaranteed to be sent at some point. (since the actions channel is reliable)
            channel.send_tick = Some(bevy_tick);
            let priority = channel.accumulated_priority;
            let message_id = channel.actions_next_send_message_id;
            channel.actions_next_send_message_id += 1;
            channel.last_action_tick = Some(tick);
            // we use SendEntityActionsMessage so that we don't have to convert the hashmap into a vec
            let message = SendEntityActionsMessage {
                sequence_id: message_id,
                group_id,
                actions,
            };
            trace!("final action messages to send: {:?}", message);

            // Since we are serializing directly though the Transport, we need to serialize the message_net_id ourselves
            actions_net_id
                .to_bytes(&mut self.writer)?;
            message
                .to_bytes(&mut self.writer)?;
            let message_bytes = self.writer.split();
            let message_id = sender
                .send_mut_with_priority::<ActionsChannel>(message_bytes, priority)?
                .expect("The entity actions channels should always return a message_id");
            debug!(
                ?message_id,
                ?group_id,
                ?bevy_tick,
                ?tick,
                "Send replication action"
            );

            // restore the hashmap that we took out, so that we can reuse the allocated memory
            channel.pending_actions = message.actions;
            channel.pending_actions.clear();

            Ok::<(), ReplicationError>(())
        })
    }

    /// Buffer the [`EntityUpdatesMessage`](super::EntityUpdatesMessage) to send in the [`MessageManager`]
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub(crate) fn send_updates_messages(
        &mut self,
        tick: Tick,
        bevy_tick: BevyTick,
        transport: &mut Transport,
        updates_net_id: MessageNetId,
    ) -> Result<(), ReplicationError> {
        self.group_with_updates.drain().try_for_each(|group_id| {
            let channel = self.group_channels.get_mut(&group_id).unwrap();
            let updates = core::mem::take(&mut channel.pending_updates);
            trace!(?group_id, "pending updates: {:?}", updates);
            let priority = channel.accumulated_priority;
            let message = UpdatesSendMessage {
                group_id,
                // TODO: as an optimization (to avoid 1 byte for the Option), we can use `last_action_tick = tick`
                //  to signify that there is no constraint!
                // SAFETY: the last action tick is usually always set because we send Actions before Updates
                //  but that might not be the case (for example if the authority got transferred to us, we start sending
                //  updates without sending any action before that)
                last_action_tick: channel.last_action_tick,
                updates,
            };

            // Since we are serializing directly though the Transport, we need to serialize the message_net_id ourselves
            updates_net_id
                .to_bytes(&mut self.writer)?;
            message
                .to_bytes(&mut self.writer)?;
            let message_bytes = self.writer.split();
            let message_id = transport
                .send_mut_with_priority::<UpdatesChannel>(message_bytes, priority)?
                .expect("The entity actions channels should always return a message_id");

            // keep track of the message_id -> group mapping, so we can handle receiving an ACK for that message_id later
            debug!(
                ?message_id,
                ?group_id,
                ?bevy_tick,
                ?tick,
                "Send replication update"
            );
            self.updates_message_id_to_group_id.insert(
                message_id,
                UpdateMessageMetadata {
                    group_id,
                    bevy_tick,
                    tick,
                    delta: core::mem::take(&mut channel.pending_delta_updates),
                },
            );
            // If we don't have a bandwidth cap, buffering a message is equivalent to sending it
            // so we can set the `send_tick` right away
            // TODO: but doesn't that mean we double send it?
            if !self.bandwidth_cap_enabled {
                channel.send_tick = Some(bevy_tick);
            }

            // restore the hashmap that we took out, so that we can reuse the allocated memory
            channel.pending_updates = message.updates;
            channel.pending_updates.clear();
            Ok(())
        })
        // TODO: also return for each message a list of the components that have delta-compression data?
    }
}

/// Channel to keep track of sending replication messages for a given Group
#[derive(Debug)]
pub struct GroupChannel {
    /// Messages that are being written. We need to hold a buffer of messages because components actions/updates
    /// are being buffered individually but we want to group them inside a message
    ///
    /// We don't put this into group_channels because we would have to iterate through all the group_channels
    /// to collect new replication messages
    pub pending_actions: EntityHashMap<Entity, EntityActions>,
    pub pending_updates: EntityHashMap<Entity, Vec<Bytes>>,
    /// List of (Entity, Component) pairs for which we write a delta update
    pub pending_delta_updates: Vec<(Entity, ComponentKind)>,

    pub actions_next_send_message_id: MessageId,

    // TODO: maybe also keep track of which Tick this bevy-tick corresponds to? (will enable doing diff-compression)
    /// Bevy Tick when we last sent an update for this group.
    /// This is used to collect updates that we will replicate; we replicate any update that happened after this tick.
    /// (and not after the last ack_tick, because 99% of the time the packet won't be lost so there is no need
    /// to wait for an ack. If we keep sending updates since the last ack, we would be sending a lot of duplicate messages)
    ///
    /// at the start, it's `None` (meaning that we send any changes)
    pub send_tick: Option<BevyTick>,
    /// Bevy Tick when we last received an ack for an update message for this group.
    ///
    /// If a message is acked, we bump the ack_tick to the `send_tick` at which we sent the update.
    /// (meaning that we don't need to send updates that happened before that `send_tick` anymore)
    ///
    /// If a message is lost, we bump the `send_tick` back to the `ack_tick`, because we might need to re-send those updates.
    pub ack_bevy_tick: Option<BevyTick>,
    /// For delta compression, we need to keep the last ack-tick that we compute the diff from
    /// for each (entity, component) pair.
    /// Keeping a tick for the entire replication group is not enough.
    /// For example:
    /// - tick 1: send C1A
    /// - tick 2: send C2. After it's received, ack_tick = 2
    /// - tick 3: send C1B as diff-C1A-C1B. The receiver cannot process it if the ack_tick = 2, because the receiver stored (C1A, tick 1) in its buffer
    ///
    /// Another solution might be that the receiver also only keeps track of a single ack tick
    /// for the entire replication group, but that needs to be fleshed out more.
    pub delta_ack_ticks: HashMap<(Entity, ComponentKind), Tick>,

    /// Last tick for which we sent an action message. Needed because we want the receiver to only
    /// process Updates if they have processed all Actions that happened before them.
    pub last_action_tick: Option<Tick>,

    /// The priority to send the replication group.
    /// This will be reset to base_priority every time we send network updates, unless we couldn't send a message
    /// for this group because of the bandwidth cap, in which case it will be accumulated.
    pub accumulated_priority: f32,
    pub base_priority: f32,
}

impl Default for GroupChannel {
    fn default() -> Self {
        Self {
            pending_updates: EntityHashMap::default(),
            pending_actions: EntityHashMap::default(),
            pending_delta_updates: Vec::default(),
            actions_next_send_message_id: MessageId(0),
            send_tick: None,
            ack_bevy_tick: None,
            delta_ack_ticks: HashMap::default(),
            last_action_tick: None,
            accumulated_priority: 0.0,
            base_priority: 1.0,
        }
    }
}
