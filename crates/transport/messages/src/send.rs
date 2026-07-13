use crate::plugin::{MessagePlugin, PendingTimelinePayloads, TimelineMessageConfig};
use crate::prelude::MessageReceiver;
use crate::registry::{MessageError, MessageKind, MessageRegistry, TimelineKind};
use crate::{Message, MessageManager, MessageNetId};
use alloc::sync::Arc;
use alloc::vec::Vec;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::{
    change_detection::MutUntyped,
    component::Component,
    entity::Entity,
    query::{With, Without},
    system::{ParallelCommands, Query, Res},
    world::{DeferredWorld, FilteredEntityMut, World},
};
use bevy_reflect::Reflect;
use bevy_utils::prelude::DebugName;
use lightyear_connection::client::Connected;
use lightyear_connection::host::HostClient;
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_serde::ToBytes;
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_serde::writer::Writer;
use lightyear_transport::channel::{Channel, ChannelKind};
use lightyear_transport::prelude::{ChannelRegistry, Transport};
#[allow(unused_imports)]
use tracing::{error, info, trace};

pub type Priority = f32;

/// A component that allows an entity to send messages of type `M` over the network.
///
/// You can send a message by simply buffering the messages into the `MessageSender<M>` component using the `send` or `send_with_priority` methods.
///
/// ```rust
/// # use bevy_ecs::prelude::*;
/// # use lightyear_messages::prelude::*;
///
/// struct M;
///
/// struct Channel;
///
/// # let mut world = World::new();
/// let mut message_sender = MessageSender::<M>::default();
/// message_sender.send::<Channel>(M);
/// ```
#[derive(Component, Reflect)]
#[component(on_add = MessageSender::<M>::on_add_hook)]
#[require(MessageManager)]
pub struct MessageSender<M: Message> {
    send: Vec<PendingSend<M>>,
    #[reflect(ignore)]
    writer: Writer,
}

struct PendingSend<M> {
    message: M,
    channel_kind: ChannelKind,
    channel_name: &'static str,
    priority: Priority,
}

// enable sending with target?
// send: Vec<(M, ChannelKind, Priority, Option<NetworkTarget>)>
// receiver can check what the intended target was.

// server send to clients:
//  server can just send on each sender. so we need a way to send with just borrowing

impl<M: Message> Default for MessageSender<M> {
    fn default() -> Self {
        Self {
            send: Vec::new(),
            writer: Writer::default(),
        }
    }
}

// SAFETY: the sender must correspond to the correct `MessageSender<M>` type
pub(crate) type SendMessageFn = unsafe fn(
    sender: MutUntyped,
    message_net_id: MessageNetId,
    transport: &Transport,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut SendEntityMap,
) -> Result<(), MessageError>;

// SAFETY: the sender must correspond to the correct `MessageSender<M>` type
// SAFETY: the receiver must correspond to the correct `MessageReceiver<M>` type
pub(crate) type SendLocalMessageFn = unsafe fn(
    sender: MutUntyped,
    receiver: MutUntyped,
    tick: Tick,
    registry: &MessageRegistry,
    channel_registry: &ChannelRegistry,
    available_timelines: &[(TimelineKind, Tick)],
    config: &TimelineMessageConfig,
) -> Result<usize, MessageError>;

impl<M: Message> MessageSender<M> {
    /// Buffers a message to be sent over the channel
    pub fn send_with_priority<C: Channel>(&mut self, message: M, priority: Priority) {
        // // TODO: how to include the sender in the metric?
        // metrics::counter!("message::send", 1,
        //     channel => core::any::type_name::<C>(),
        //     message => core::any::type_name::<M>()
        // );
        self.send.push(PendingSend {
            message,
            channel_kind: ChannelKind::of::<C>(),
            channel_name: core::any::type_name::<C>(),
            priority,
        });
    }

    /// Buffers a message to be sent over the channel
    pub fn send<C: Channel>(&mut self, message: M) {
        self.send_with_priority::<C>(message, 1.0);
    }

    /// Take all messages from the [`MessageSender<M>`], serialize them, and buffer them
    /// on the appropriate channel of the [`Transport`].
    ///
    /// SAFETY: the `message_sender` must be of type [`MessageSender<M>`]
    pub(crate) unsafe fn send_message_typed(
        message_sender: MutUntyped,
        net_id: MessageNetId,
        transport: &Transport,
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY:  the `message_sender` must be of type `MessageSender<M>`
        let mut sender = unsafe { message_sender.with_type::<Self>() };
        // enable split borrows
        let sender = &mut *sender;
        let pending = core::mem::take(&mut sender.send);
        let mut pending = pending.into_iter();
        while let Some(item) = pending.next() {
            // we write the message NetId, and then serialize the message
            let serialize_result = net_id.to_bytes(&mut sender.writer).and_then(|_| {
                // SAFETY: the message has been checked to be of type `M`.
                unsafe {
                    serialize_metadata.serialize::<SendEntityMap, M, M>(
                        &item.message,
                        &mut sender.writer,
                        entity_map,
                    )
                }
            });
            if let Err(error) = serialize_result {
                sender.writer.split();
                sender.send.push(item);
                sender.send.extend(pending);
                return Err(error.into());
            }
            let bytes = sender.writer.split();
            #[cfg(feature = "metrics")]
            {
                metrics::counter!("message/send", "message" => core::any::type_name::<M>())
                    .increment(1);
                metrics::gauge!("message/send_bytes", "message" => core::any::type_name::<M>())
                    .increment(bytes.len() as f64);
            }
            trace!(
                "Sending message of type {:?} with net_id {net_id:?}/kind {:?} on channel {:?}",
                DebugName::type_name::<M>(),
                MessageKind::of::<M>(),
                item.channel_kind
            );
            trace!(
                target: "lightyear_debug::message",
                kind = "message_send",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                message_name = core::any::type_name::<M>(),
                message_net_id = net_id,
                channel = item.channel_name,
                bytes = bytes.len(),
                priority = item.priority,
                "serialized message for transport"
            );
            if let Err(error) = transport.send_erased(item.channel_kind, bytes, item.priority) {
                sender.send.push(item);
                sender.send.extend(pending);
                return Err(error.into());
            }
        }
        Ok(())
    }

    /// Take all messages from the [`MessageSender<M>`], and add them to [`MessageReceiver<M>`]
    ///
    /// # Safety
    /// - the `message_sender` must be of type [`MessageSender<M>`]
    /// - the `message_receiver` must be of type [`MessageReceiver<M>`]
    pub(crate) unsafe fn send_local_message_typed(
        message_sender: MutUntyped,
        message_receiver: MutUntyped,
        tick: Tick,
        registry: &MessageRegistry,
        channel_registry: &ChannelRegistry,
        available_timelines: &[(TimelineKind, Tick)],
        config: &TimelineMessageConfig,
    ) -> Result<usize, MessageError> {
        // SAFETY:  the `message_sender` must be of type `MessageSender<M>`
        let mut sender = unsafe { message_sender.with_type::<Self>() };
        // SAFETY:  the `message_receiver` must be of type `MessageReceiver<M>`
        let mut receiver = unsafe { message_receiver.with_type::<MessageReceiver<M>>() };
        // enable split borrows
        let sender = &mut *sender;
        let queued = core::mem::take(&mut sender.send);
        let mut queued = queued.into_iter();
        let mut pending_count = 0;
        while let Some(pending) = queued.next() {
            trace!(
                "Send local message of type {:?} on channel {:?}",
                DebugName::type_name::<M>(),
                pending.channel_kind
            );
            trace!(
                target: "lightyear_debug::message",
                kind = "message_send_local",
                schedule = "Last",
                sample_point = "Last",
                message_name = core::any::type_name::<M>(),
                channel = pending.channel_name,
                remote_tick = tick.0,
                "queued local message"
            );
            let target_timeline = channel_registry
                .settings(pending.channel_kind)
                .and_then(|settings| settings.delivery_timeline())
                .map(TimelineKind::from);
            if let Some(timeline) = target_timeline {
                if !registry.timeline_metadata.contains_key(&timeline) {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::TimelineNotRegistered(timeline));
                }
                let Some((_, current_tick)) = available_timelines
                    .iter()
                    .find(|(kind, _)| *kind == timeline)
                else {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::MissingTimeline(timeline));
                };
                let delta = tick - *current_tick;
                if delta > 0 && delta as u32 > config.max_future_ticks {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::TimelineTooFarAhead {
                        target: tick,
                        current: *current_tick,
                        max_future_ticks: config.max_future_ticks,
                    });
                }
                if let Err(error) = receiver.ensure_pending_capacity(config) {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(error);
                }
            }
            if let Err(error) = receiver.push_received(
                pending.message,
                tick,
                pending.channel_kind,
                None,
                target_timeline,
                config,
            ) {
                sender.send.extend(queued);
                return Err(error);
            }
            pending_count += usize::from(target_timeline.is_some());
        }
        Ok(pending_count)
    }

    pub fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let mut entity_mut = world.entity_mut(context.entity);
            let mut message_manager = entity_mut.get_mut::<MessageManager>().unwrap();
            let message_kind_present = message_manager
                .send_messages
                .iter()
                .any(|(message_kind, _)| *message_kind == MessageKind::of::<M>());
            if !message_kind_present {
                message_manager
                    .send_messages
                    .push((MessageKind::of::<M>(), context.component_id));
            }
        })
    }
}

impl MessagePlugin {
    /// Take messages to send from the [`MessageSender<M>`] components
    /// Serialize them into bytes that are buffered in a [`Transport`]
    pub fn send(
        mut transport_query: Query<
            (Entity, &Transport, &mut MessageManager),
            (With<Connected>, Without<HostClient>),
        >,
        // MessageSender<M> present on that entity
        message_sender_query: Query<FilteredEntityMut>,
        registry: Res<MessageRegistry>,
    ) {
        // We use Arc to make the query Clone, since we know that we will only access MessageSender<M> components
        // on different entities
        let message_sender_query = Arc::new(message_sender_query);
        transport_query
            .par_iter_mut()
            .for_each(|(entity, transport, mut message_manager)| {
                // SAFETY: we know that this won't lead to violating the aliasing rule
                let mut message_sender_query = unsafe { message_sender_query.reborrow_unsafe() };

                // TODO: allow sending from senders in parallel! The only issue is the mutable borrow of the entity mapper
                // enable split borrows
                let message_manager = &mut *message_manager;
                message_manager
                    .send_messages
                    .iter()
                    .try_for_each(|(message_kind, sender_id)| {
                        let mut entity_mut = message_sender_query.get_mut(entity).unwrap();
                        let message_sender = entity_mut
                            .get_mut_by_id(*sender_id)
                            .ok_or(MessageError::MissingComponent(*sender_id))?;
                        let send_metadata = registry
                            .send_metadata
                            .get(message_kind)
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                        let serialize_fns = registry
                            .serialize_fns_map
                            .get(message_kind)
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                        let message_id = registry
                            .kind_map
                            .net_id(message_kind)
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                        // SAFETY: we know the message_sender corresponds to the correct `MessageSender<M>` type
                        unsafe {
                            (send_metadata.send_message_fn)(
                                message_sender,
                                *message_id,
                                transport,
                                serialize_fns,
                                &mut message_manager.entity_mapper.local_to_remote,
                            )?;
                        }
                        Ok::<_, MessageError>(())
                    })
                    .inspect_err(|e| error!("error sending message: {e:?}"))
                    .ok();

                // TODO: allow sending from senders in parallel! The only issue is the mutable borrow of the entity mapper
                // enable split borrows
                let message_manager = &mut *message_manager;
                message_manager
                    .send_triggers
                    .iter()
                    .try_for_each(|(message_kind, sender_id)| {
                        let mut entity_mut = message_sender_query.get_mut(entity).unwrap();
                        let message_sender = entity_mut
                            .get_mut_by_id(*sender_id)
                            .ok_or(MessageError::MissingComponent(*sender_id))?;
                        let send_metadata = registry
                            .send_trigger_metadata
                            .get(message_kind)
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                        let serialize_fns = registry
                            .serialize_fns_map
                            .get(message_kind)
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                        let message_id = registry
                            .kind_map
                            .net_id(message_kind)
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                        // SAFETY: we know the message_sender corresponds to the correct `MessageSender<M>` type
                        unsafe {
                            (send_metadata.send_trigger_fn)(
                                message_sender,
                                *message_id,
                                transport,
                                serialize_fns,
                                &mut message_manager.entity_mapper.local_to_remote,
                            )?;
                        }
                        Ok::<_, MessageError>(())
                    })
                    .inspect_err(|e| error!("error sending trigger: {e:?}"))
                    .ok();
            })
    }

    /// For the host-client, we take messages to send from the [`MessageSender<M>`] components
    /// and add them directly to the [`MessageReceiver<M>`] components.
    /// (the [`Transport`] is not used)
    pub fn send_local(
        timeline: Res<LocalTimeline>,
        mut manager_query: Query<
            (Entity, &mut MessageManager),
            (With<Connected>, With<HostClient>),
        >,
        // MessageSender<M>/MessageReceiver<M>/TriggerSender<M> present on that entity
        message_components_query: Query<FilteredEntityMut>,
        commands: ParallelCommands,
        registry: Res<MessageRegistry>,
        channel_registry: Res<ChannelRegistry>,
        config: Res<TimelineMessageConfig>,
    ) {
        // We use Arc to make the query Clone, since we know that we will only access MessageSender<M>/MessageReceiver<M> components
        // on different entities
        let tick = timeline.tick();
        let message_components_query = Arc::new(message_components_query);
        manager_query
            .par_iter_mut()
            .for_each(|(entity, mut message_manager)| {
                // SAFETY: we know that this won't lead to violating the aliasing rule
                let mut message_sender_query =
                    unsafe { message_components_query.reborrow_unsafe() };
                let mut message_receiver_query =
                    unsafe { message_components_query.reborrow_unsafe() };

                // TODO: allow sending from senders in parallel! The only issue is the mutable borrow of the entity mapper
                // enable split borrows
                let message_manager = &mut *message_manager;
                let available_timelines = registry
                    .timeline_metadata
                    .iter()
                    .filter_map(|(kind, metadata)| {
                        let entity = message_components_query.get(entity).ok()?;
                        let timeline = entity.get_by_id(metadata.component_id)?;
                        // SAFETY: the callback is registered with this timeline component id.
                        Some((*kind, unsafe { (metadata.tick_fn)(timeline) }))
                    })
                    .collect::<Vec<_>>();
                message_manager
                    .send_messages
                    .iter()
                    .try_for_each(|(message_kind, sender_id)| {
                        let mut entity_mut = message_sender_query.get_mut(entity).unwrap();
                        let message_sender = entity_mut
                            .get_mut_by_id(*sender_id)
                            .ok_or(MessageError::MissingComponent(*sender_id))?;
                        // TODO: maybe use an IndexMap for faster lookup?
                        let receiver_id = message_manager
                            .receive_messages
                            .iter()
                            .find_map(
                                |(kind, id)| if kind == message_kind { Some(id) } else { None },
                            )
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;

                        let mut entity_mut = message_receiver_query.get_mut(entity).unwrap();
                        let message_receiver = entity_mut
                            .get_mut_by_id(*receiver_id)
                            .ok_or(MessageError::MissingComponent(*receiver_id))?;
                        let send_metadata = registry
                            .send_metadata
                            .get(message_kind)
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                        // SAFETY: we know the message_sender corresponds to the correct `MessageSender<M>` type
                        unsafe {
                            let pending_count = (send_metadata.send_local_message_fn)(
                                message_sender,
                                message_receiver,
                                tick,
                                &registry,
                                &channel_registry,
                                &available_timelines,
                                &config,
                            )?;
                            if pending_count != 0 {
                                commands.command_scope(|mut commands| {
                                    commands.entity(entity).insert(PendingTimelinePayloads);
                                });
                            }
                        }
                        Ok::<_, MessageError>(())
                    })
                    .inspect_err(|e| error!("error sending message on host-client: {e:?}"))
                    .ok();

                // TODO: allow sending from senders in parallel! The only issue is the mutable borrow of the entity mapper
                // enable split borrows
                let message_manager = &mut *message_manager;
                message_manager
                    .send_triggers
                    .iter()
                    .try_for_each(|(message_kind, sender_id)| {
                        let mut entity_mut = message_sender_query.get_mut(entity).unwrap();
                        let message_sender = entity_mut
                            .get_mut_by_id(*sender_id)
                            .ok_or(MessageError::MissingComponent(*sender_id))?;
                        let receiver_id =
                            message_manager
                                .receive_triggers
                                .iter()
                                .find_map(
                                    |(kind, id)| if kind == message_kind { Some(id) } else { None },
                                );
                        let send_metadata = registry
                            .send_trigger_metadata
                            .get(message_kind)
                            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                        if let Some(receiver_id) = receiver_id {
                            let mut entity_mut = message_receiver_query.get_mut(entity).unwrap();
                            let message_receiver = entity_mut
                                .get_mut_by_id(*receiver_id)
                                .ok_or(MessageError::MissingComponent(*receiver_id))?;
                            // SAFETY: the sender and receiver component ids come from the message registry
                            // for this event type.
                            unsafe {
                                let pending_count = (send_metadata.send_local_trigger_fn)(
                                    message_sender,
                                    Some(message_receiver),
                                    &commands,
                                    tick,
                                    &registry,
                                    &channel_registry,
                                    &available_timelines,
                                    &config,
                                )?;
                                if pending_count != 0 {
                                    commands.command_scope(|mut commands| {
                                        commands.entity(entity).insert(PendingTimelinePayloads);
                                    });
                                }
                            }
                        } else {
                            // SAFETY: the sender component id comes from the message registry for this event type.
                            unsafe {
                                let pending_count = (send_metadata.send_local_trigger_fn)(
                                    message_sender,
                                    None,
                                    &commands,
                                    tick,
                                    &registry,
                                    &channel_registry,
                                    &available_timelines,
                                    &config,
                                )?;
                                debug_assert_eq!(pending_count, 0);
                            }
                        }
                        Ok::<_, MessageError>(())
                    })
                    .inspect_err(|e| error!("error sending trigger on host-client: {e:?}"))
                    .ok();
            })
    }
}
