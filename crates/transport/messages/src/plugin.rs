use crate::MessageManager;
use crate::receive::BufferedMessageTimeline;
use crate::registry::{MessageRegistry, ReceiveTriggerMetadata};
use bevy_app::{App, Last, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::{Add, On, Resource, With};
use bevy_ecs::{
    schedule::{ApplyDeferred, IntoScheduleConfigs, SystemSet},
    system::{ParallelCommands, ParamBuilder, Query, QueryParamBuilder, Res, SystemParamBuilder},
    world::{DeferredWorld, FilteredEntityMut},
};
use lightyear_connection::client::{Connected, Disconnected};
use lightyear_core::prelude::{NetworkTimeline, TimelineSystems};
use lightyear_transport::plugin::{TransportPlugin, TransportSystems};

#[deprecated(note = "Use MessageSystems instead")]
pub type MessageSet = MessageSystems;

/// System sets for the typed-message pipeline.
///
/// [`Receive`](Self::Receive) is the public ordering boundary for the complete
/// receive pipeline. It contains these two ordered subsets:
///
/// 1. [`ReceiveMessages`](Self::ReceiveMessages) deserializes incoming payloads.
/// 2. [`ReleaseTimeline`](Self::ReleaseTimeline) releases timeline-ready payloads.
///
/// Most application systems should order against [`Receive`](Self::Receive).
/// Order against one of its subsets only when a system intentionally needs to
/// run between the two receive phases.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum MessageSystems {
    /// Public outer set for the complete typed-message receive pipeline.
    ///
    /// Prefer ordering application systems before or after this set. Systems
    /// ordered after it run after both deserialization and timeline release.
    Receive,

    /// First internal subset of [`Receive`](Self::Receive).
    ///
    /// Receives bytes from transport, deserializes them, and buffers them in
    /// the appropriate typed message/event receiver. Most users should order
    /// against [`Receive`](Self::Receive), not this individual phase.
    ReceiveMessages,

    /// Second internal subset of [`Receive`](Self::Receive).
    ///
    /// Releases messages and events whose requested connection timeline has
    /// reached the sender tick. Most users should order against
    /// [`Receive`](Self::Receive), not this individual phase.
    ReleaseTimeline,

    // PostUpdate
    /// Receive messages from the [`MessageSender<M>`](crate::send::MessageSender), serialize them into Bytes
    /// and buffer those in the Transport
    Send,
}

// PLUGIN
// recv-messages: query all Transport + MessageManager
//  MessageManager is similar to transport, it holds references to MessageReceiver<M> and MessageSender<M> component ids
pub struct MessagePlugin;

/// Bounds memory and latency for payloads waiting on a delivery timeline.
#[derive(Resource, Clone, Copy, Debug)]
pub struct TimelineMessageConfig {
    /// Maximum number of timeline-delayed payloads in one typed receiver.
    pub max_pending_per_receiver: usize,
    /// Maximum number of ticks a payload may target ahead of the receiver timeline.
    pub max_future_ticks: u32,
}

impl Default for TimelineMessageConfig {
    fn default() -> Self {
        Self {
            max_pending_per_receiver: 4096,
            max_future_ticks: 100,
        }
    }
}

/// Registers `T` as a timeline that can be targeted by typed messages/events.
///
/// Timeline registration is part of the network protocol and must happen in
/// the same order on both peers, before connections are spawned.
/// Registration records type-erased metadata only: it never adds `T` to an
/// entity. Each receiving connection that accepts a channel targeting `T`
/// must already contain its own `T` component.
pub fn register_message_timeline<T: NetworkTimeline + BufferedMessageTimeline>(app: &mut App) {
    if !app.world().contains_resource::<MessageRegistry>() {
        app.world_mut().init_resource::<MessageRegistry>();
    }
    let component_id = app.world_mut().register_component::<T>();
    app.world_mut()
        .resource_mut::<MessageRegistry>()
        .register_timeline::<T>(component_id);
}

impl MessagePlugin {
    // TODO: do something similar to Transport? (use observers instead of required_components)?
    /// On disconnect:
    /// - Reset the MessageManager to its original state
    fn handle_disconnection(trigger: On<Add, Disconnected>, mut world: DeferredWorld) {
        let receiver_components = {
            let Some(mut manager) = world.get_mut::<MessageManager>(trigger.entity) else {
                return;
            };
            manager.entity_mapper.clear();
            manager.receive_messages.clone()
        };
        let pending_receivers = {
            let Some(registry) = world.get_resource::<MessageRegistry>() else {
                return;
            };
            let mut pending_receivers = receiver_components
                .into_iter()
                .filter_map(|(kind, component_id)| {
                    registry
                        .receive_metadata
                        .get(&kind)
                        .and_then(|metadata| metadata.timeline)
                        .map(|timeline| (component_id, timeline.clear_fn))
                })
                .collect::<alloc::vec::Vec<_>>();
            pending_receivers.extend(registry.receive_trigger.values().filter_map(|metadata| {
                let ReceiveTriggerMetadata::Timeline(metadata) = metadata else {
                    return None;
                };
                Some((metadata.component_id, metadata.clear_fn))
            }));
            pending_receivers
        };
        for (component_id, clear) in pending_receivers {
            if let Some(receiver) = world.get_mut_by_id(trigger.entity, component_id) {
                // SAFETY: the callback is registered with this receiver component id.
                unsafe { clear(receiver) };
            }
        }
    }

    /// Releases pending messages and events against timelines stored on the
    /// same connected entity.
    ///
    /// Each registered timeline is read once per connection. Only payloads
    /// targeting that timeline and whose sender tick is now visible are moved
    /// into the normal receive buffer or triggered as remote events.
    fn release_timeline(
        mut entities: Query<(&MessageManager, FilteredEntityMut), With<Connected>>,
        registry: Res<MessageRegistry>,
        commands: ParallelCommands,
    ) {
        for (manager, mut entity) in entities.iter_mut() {
            for (timeline_kind, timeline_metadata) in &registry.timeline_metadata {
                let Some(timeline) = entity.get_by_id(timeline_metadata.component_id) else {
                    continue;
                };
                // SAFETY: tick_fn is registered together with this component id.
                let tick = unsafe { (timeline_metadata.tick_fn)(timeline) };

                for (kind, component_id) in &manager.receive_messages {
                    if kind.timeline != Some(*timeline_kind) {
                        continue;
                    }
                    if let Some(receiver) = entity.get_mut_by_id(*component_id) {
                        let Some(metadata) = registry.receive_metadata.get(kind) else {
                            continue;
                        };
                        let Some(timeline) = metadata.timeline else {
                            continue;
                        };
                        // SAFETY: the callback is registered for this receiver component id.
                        unsafe { (timeline.release_fn)(receiver, tick) };
                    }
                }

                for (kind, metadata) in &registry.receive_trigger {
                    if kind.timeline != Some(*timeline_kind) {
                        continue;
                    }
                    let ReceiveTriggerMetadata::Timeline(metadata) = metadata else {
                        continue;
                    };
                    if let Some(receiver) = entity.get_mut_by_id(metadata.component_id) {
                        // SAFETY: the callback is registered for this receiver component id.
                        unsafe { (metadata.release_fn)(receiver, &commands, tick) };
                    }
                }
            }
        }
    }
}

impl Plugin for MessagePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TransportPlugin>() {
            app.add_plugins(TransportPlugin);
        }
        app.init_resource::<TimelineMessageConfig>();

        app.add_observer(Self::handle_disconnection);

        #[cfg(feature = "client")]
        app.register_required_components::<lightyear_connection::client::Client, MessageManager>();

        #[cfg(feature = "server")]
        app.register_required_components::<lightyear_connection::prelude::server::ClientOf, MessageManager>();
    }

    // NOTE: this should only be called once all messages are registered, because we use the list of registered
    //  messages to provide the dynamic access
    fn finish(&self, app: &mut App) {
        let registry = app
            .world_mut()
            .remove_resource::<MessageRegistry>()
            .unwrap();

        let recv = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.receive_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                    registry.receive_trigger.values().for_each(|metadata| {
                        if let ReceiveTriggerMetadata::Timeline(metadata) = metadata {
                            b.mut_id(metadata.component_id);
                        }
                    });
                    registry.timeline_metadata.values().for_each(|metadata| {
                        b.ref_id(metadata.component_id);
                    });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::recv)
            .with_name("MessagePlugin::recv");

        let clear = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.receive_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                });
            }),
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::clear)
            .with_name("MessagePlugin::clear");

        let send = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.send_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                    registry
                        .send_trigger_metadata
                        .values()
                        .for_each(|metadata| {
                            b.mut_id(metadata.component_id);
                        });
                });
            }),
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::send)
            .with_name("MessagePlugin::send");

        let send_local = (
            ParamBuilder,
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.send_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                    registry.receive_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                    registry.receive_trigger.values().for_each(|metadata| {
                        if let ReceiveTriggerMetadata::Timeline(metadata) = metadata {
                            b.mut_id(metadata.component_id);
                        }
                    });
                    registry
                        .send_trigger_metadata
                        .values()
                        .for_each(|metadata| {
                            b.mut_id(metadata.component_id);
                        });
                    registry.timeline_metadata.values().for_each(|metadata| {
                        b.ref_id(metadata.component_id);
                    });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::send_local)
            .with_name("MessagePlugin::send_local");

        let release_timeline = (
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.receive_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                    registry.receive_trigger.values().for_each(|metadata| {
                        if let ReceiveTriggerMetadata::Timeline(metadata) = metadata {
                            b.mut_id(metadata.component_id);
                        }
                    });
                    registry.timeline_metadata.values().for_each(|metadata| {
                        b.ref_id(metadata.component_id);
                    });
                });
            }),
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::release_timeline)
            .with_name("MessagePlugin::release_timeline");

        app.configure_sets(
            PreUpdate,
            MessageSystems::Receive.after(TransportSystems::Receive),
        );
        app.configure_sets(
            PreUpdate,
            (
                MessageSystems::ReceiveMessages,
                MessageSystems::ReleaseTimeline,
            )
                .chain()
                .in_set(MessageSystems::Receive),
        );
        app.configure_sets(
            PreUpdate,
            MessageSystems::ReleaseTimeline.after(TimelineSystems::Advance),
        );
        app.configure_sets(
            PostUpdate,
            MessageSystems::Send.before(TransportSystems::Send),
        );
        app.add_systems(PreUpdate, recv.in_set(MessageSystems::ReceiveMessages));
        app.add_systems(
            PreUpdate,
            ApplyDeferred
                .after(MessageSystems::ReceiveMessages)
                .before(MessageSystems::ReleaseTimeline)
                .in_set(MessageSystems::Receive),
        );
        app.add_systems(
            PreUpdate,
            release_timeline.in_set(MessageSystems::ReleaseTimeline),
        );
        app.add_systems(PostUpdate, send.in_set(MessageSystems::Send));
        // we need to send local messages after clear, otherwise they will be cleared immediately after sending
        app.add_systems(Last, (clear, send_local).chain());

        app.world_mut().insert_resource(registry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receive::{BufferedMessageTimeline, MessageReceiver};
    use crate::receive_event::{PendingTimelineEvents, RemoteEvent};
    use crate::registry::AppMessageExt;
    use crate::send::MessageSender;
    use crate::send_trigger::EventSender;
    use crate::trigger::AppTriggerExt;
    use alloc::{vec, vec::Vec};
    use bevy_ecs::event::Event;
    use bevy_ecs::prelude::{Component, Entity, ResMut, Resource};
    use lightyear_connection::client::{Client, Connected};
    use lightyear_connection::direction::NetworkDirection;
    use lightyear_connection::host::HostClient;
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_core::plugin::CorePlugins;
    use lightyear_core::prelude::{LocalTimeline, NetworkTimeline, Tick};
    use lightyear_core::time::{Overstep, TickDelta, TickInstant};
    use lightyear_core::timeline::TimelineConfig;
    use lightyear_link::{Link, Linked};
    use lightyear_transport::channel::ChannelKind;
    use lightyear_transport::plugin::TestChannel;
    use lightyear_transport::plugin::TestTransportPlugin;
    use lightyear_transport::prelude::{
        AppChannelExt, ChannelRegistry, ChannelSettings, Transport,
    };
    use serde::{Deserialize, Serialize};
    use test_log::test;

    /// Message
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    struct M(usize);

    struct TimelineChannel;

    #[derive(Event, Serialize, Deserialize, Clone, Debug, PartialEq)]
    struct E(usize);

    #[derive(Component)]
    struct TestTimelineConfig;

    #[derive(Component, Default)]
    struct TestTimeline(TickInstant);

    impl TimelineConfig for TestTimelineConfig {
        type Context = ();
        type Timeline = TestTimeline;
    }

    impl NetworkTimeline for TestTimeline {
        type Config = TestTimelineConfig;

        fn now(&self) -> TickInstant {
            self.0
        }

        fn tick(&self) -> Tick {
            self.0.tick()
        }

        fn overstep(&self) -> Overstep {
            self.0.overstep()
        }

        fn set_now(&mut self, now: TickInstant) {
            self.0 = now;
        }

        fn apply_delta(&mut self, delta: TickDelta) {
            self.0 = self.0 + delta;
        }
    }

    impl BufferedMessageTimeline for TestTimeline {}

    #[derive(Resource, Default)]
    struct EventCount(usize);

    #[derive(Resource, Default)]
    struct ObservedAfterReceive(bool);

    fn count_event(_: On<RemoteEvent<E>>, mut count: ResMut<EventCount>) {
        count.0 += 1;
    }

    fn observe_after_receive(
        receivers: Query<&MessageReceiver<M, TestTimeline>>,
        mut observed: ResMut<ObservedAfterReceive>,
    ) {
        observed.0 = receivers.iter().any(MessageReceiver::has_messages);
    }

    fn message_test_app(register_event: bool) -> App {
        let mut app = App::new();
        app.add_plugins(CorePlugins {
            tick_duration: core::time::Duration::from_millis(10),
        });
        app.add_plugins(TestTransportPlugin);
        app.add_channel::<TimelineChannel>(
            ChannelSettings::default().with_timeline::<TestTimeline>(),
        );
        app.init_resource::<lightyear_connection::client::PeerMetadata>();
        register_message_timeline::<TestTimeline>(&mut app);
        app.register_message::<M>()
            .add_direction_on_timeline::<TestTimeline>(NetworkDirection::Bidirectional);
        if register_event {
            app.register_event::<E>()
                .add_direction_on_timeline::<TestTimeline>(NetworkDirection::Bidirectional);
            app.init_resource::<EventCount>();
            app.add_observer(count_event);
        }
        app.add_plugins(MessagePlugin);
        app.finish();
        app
    }

    fn loopback_link_payload(app: &mut App, entity: Entity) {
        let mut entity_mut = app.world_mut().entity_mut(entity);
        let mut link = entity_mut.get_mut::<Link>().unwrap();
        let payload = link.send.pop().expect("expected one outgoing payload");
        link.recv.push_raw(payload);
    }

    // TODO: should we do a test without the Link?

    /// Check that if we have a Transport, we can send and receive messages to specific channels
    #[test]
    fn test_send_receive() {
        let mut app = App::new();
        app.add_plugins(CorePlugins {
            tick_duration: core::time::Duration::from_millis(10),
        });
        app.add_plugins(TestTransportPlugin);

        // Register the message before adding the MessagePlugin
        app.register_message::<M>();
        app.add_plugins(MessagePlugin);
        app.finish();

        // Add the Transport component with a receiver/sender for channel C, and a receiver/sender for message M
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<TestChannel>(registry);
        transport.add_receiver_from_registry::<TestChannel>(registry);
        // TODO: are these tests useful? they need so many components from other plugins..
        let mut entity_mut = app.world_mut().spawn((
            Link::default(),
            transport,
            MessageSender::<M>::default(),
            RemoteId(PeerId::Local(0)),
            Linked,
            Connected,
        ));

        let entity = entity_mut.id();

        // send message
        let messages = vec![M(2), M(3)];
        let mut sender = entity_mut.get_mut::<MessageSender<M>>().unwrap();
        for message in messages.clone() {
            sender.send::<TestChannel>(message);
        }
        app.update();

        // check that the send-payload was added to the Transport
        let mut entity_mut = app.world_mut().entity_mut(entity);
        let mut link = entity_mut.get_mut::<Link>().unwrap();
        assert_eq!(link.send.len(), 1);

        // transfer that payload to the recv side of the link
        let payload = link.send.pop().unwrap();
        link.recv.push_raw(payload);

        app.world_mut().run_schedule(PreUpdate);

        // check that the message has been received
        let received_messages = app
            .world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M>>()
            .unwrap()
            .receive()
            .collect::<Vec<_>>();
        assert_eq!(messages, received_messages);

        app.update();

        // check that the message has been dropped
        assert!(
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<MessageReceiver<M>>()
                .unwrap()
                .num_messages()
                == 0
        );
    }

    /// Check that messages are cleared even if not read
    #[test]
    fn test_clear() {
        let mut app = App::new();
        app.add_plugins(CorePlugins {
            tick_duration: core::time::Duration::from_millis(10),
        });
        app.register_message::<M>();
        app.add_plugins(MessagePlugin);
        app.finish();

        let entity_mut = app.world_mut().spawn((
            MessageReceiver::<M>::default(),
            RemoteId(PeerId::Local(0)),
            Connected,
        ));

        let entity = entity_mut.id();

        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M>>()
            .unwrap()
            .push_received(
                M(2),
                Tick::default(),
                ChannelKind::of::<TestChannel>(),
                None,
                &TimelineMessageConfig::default(),
            )
            .unwrap();
        app.update();

        // check that the message has been dropped
        assert!(
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<MessageReceiver<M>>()
                .unwrap()
                .num_messages()
                == 0
        );
    }

    #[test]
    fn timeline_message_waits_for_the_same_entity_timeline() {
        let mut app = message_test_app(false);
        app.init_resource::<ObservedAfterReceive>();
        app.add_systems(
            PreUpdate,
            observe_after_receive.after(MessageSystems::Receive),
        );
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<TimelineChannel>(registry);
        transport.add_receiver_from_registry::<TimelineChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                MessageSender::<M>::default(),
                TestTimeline(Tick(5).into()),
                RemoteId(PeerId::Local(0)),
                Linked,
                Connected,
            ))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TimelineChannel>(M(7));

        app.update();
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);
        let receiver = app
            .world()
            .entity(entity)
            .get::<MessageReceiver<M, TestTimeline>>()
            .unwrap();
        assert_eq!(receiver.num_messages(), 0);
        assert_eq!(receiver.num_pending_timeline_messages(), 1);
        assert!(!app.world().resource::<ObservedAfterReceive>().0);

        app.world_mut()
            .entity_mut(entity)
            .get_mut::<TestTimeline>()
            .unwrap()
            .set_now(Tick(10).into());
        app.world_mut().run_schedule(PreUpdate);
        assert!(app.world().resource::<ObservedAfterReceive>().0);
        let message = app
            .world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M, TestTimeline>>()
            .unwrap()
            .receive()
            .next();
        assert_eq!(message, Some(M(7)));
    }

    #[test]
    fn lazy_timeline_receiver_releases_in_the_same_receive_phase() {
        let mut app = message_test_app(false);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<TimelineChannel>(registry);
        transport.add_receiver_from_registry::<TimelineChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                MessageSender::<M>::default(),
                TestTimeline(Tick(10).into()),
                RemoteId(PeerId::Local(0)),
                Linked,
                Connected,
            ))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TimelineChannel>(M(12));

        app.update();
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);

        let message = app
            .world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M, TestTimeline>>()
            .unwrap()
            .receive()
            .next();
        assert_eq!(message, Some(M(12)));
    }

    #[test]
    fn ordinary_message_is_immediate_even_with_a_timeline_present() {
        let mut app = message_test_app(false);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<TestChannel>(registry);
        transport.add_receiver_from_registry::<TestChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                MessageSender::<M>::default(),
                TestTimeline(Tick(1).into()),
                RemoteId(PeerId::Local(0)),
                Linked,
                Connected,
            ))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TestChannel>(M(8));
        app.update();
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);

        let mut entity_mut = app.world_mut().entity_mut(entity);
        let mut receiver = entity_mut.get_mut::<MessageReceiver<M>>().unwrap();
        assert_eq!(receiver.receive().next(), Some(M(8)));
        assert_eq!(receiver.num_pending_timeline_messages(), 0);
    }

    #[test]
    fn timeline_channel_message_is_rejected_when_receiver_lacks_timeline() {
        let mut app = message_test_app(false);
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<TimelineChannel>(registry);
        transport.add_receiver_from_registry::<TimelineChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                MessageSender::<M>::default(),
                RemoteId(PeerId::Local(0)),
                Linked,
                Connected,
            ))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TimelineChannel>(M(9));
        app.update();
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);

        assert!(
            !app.world()
                .entity(entity)
                .contains::<MessageReceiver<M, TestTimeline>>()
        );
    }

    #[test]
    fn host_client_message_uses_its_local_timeline() {
        let mut app = message_test_app(false);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);
        let entity = app
            .world_mut()
            .spawn((
                MessageSender::<M>::default(),
                TestTimeline(Tick(5).into()),
                HostClient { buffer: Vec::new() },
                RemoteId(PeerId::Local(0)),
                Connected,
            ))
            .id();
        // Apply the sender hook so MessageManager knows about it.
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TimelineChannel>(M(10));
        app.update();
        let receiver = app
            .world()
            .entity(entity)
            .get::<MessageReceiver<M, TestTimeline>>()
            .unwrap();
        assert_eq!(receiver.num_pending_timeline_messages(), 1);
        assert_eq!(receiver.num_messages(), 0);

        app.world_mut()
            .entity_mut(entity)
            .get_mut::<TestTimeline>()
            .unwrap()
            .set_now(Tick(10).into());
        app.world_mut().run_schedule(PreUpdate);
        let mut entity_mut = app.world_mut().entity_mut(entity);
        assert_eq!(
            entity_mut
                .get_mut::<MessageReceiver<M, TestTimeline>>()
                .unwrap()
                .receive()
                .next(),
            Some(M(10))
        );
    }

    #[test]
    fn host_client_lazily_adds_the_timeline_receiver() {
        let mut app = message_test_app(false);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);
        let entity = app
            .world_mut()
            .spawn((
                MessageSender::<M>::default(),
                TestTimeline(Tick(5).into()),
                HostClient { buffer: Vec::new() },
                RemoteId(PeerId::Local(0)),
                Connected,
            ))
            .id();
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TimelineChannel>(M(11));

        assert!(
            !app.world()
                .entity(entity)
                .contains::<MessageReceiver<M, TestTimeline>>()
        );

        app.update();

        let receiver = app
            .world()
            .entity(entity)
            .get::<MessageReceiver<M, TestTimeline>>()
            .unwrap();
        assert_eq!(receiver.num_pending_timeline_messages(), 1);
    }

    #[test]
    fn host_client_rejects_far_future_messages_without_losing_the_queue() {
        let mut app = message_test_app(false);
        *app.world_mut().resource_mut::<TimelineMessageConfig>() = TimelineMessageConfig {
            max_future_ticks: 2,
            ..Default::default()
        };
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(10);
        let entity = app
            .world_mut()
            .spawn((
                MessageSender::<M>::default(),
                TestTimeline(Tick(5).into()),
                HostClient { buffer: Vec::new() },
                RemoteId(PeerId::Local(0)),
                Connected,
            ))
            .id();
        app.world_mut().flush();
        {
            let mut entity_mut = app.world_mut().entity_mut(entity);
            let mut sender = entity_mut.get_mut::<MessageSender<M>>().unwrap();
            sender.send::<TimelineChannel>(M(1));
            sender.send::<TimelineChannel>(M(2));
        }

        app.update();
        assert!(
            !app.world()
                .entity(entity)
                .contains::<MessageReceiver<M, TestTimeline>>()
        );

        app.world_mut()
            .entity_mut(entity)
            .get_mut::<TestTimeline>()
            .unwrap()
            .set_now(Tick(10).into());
        app.update();
        app.world_mut().run_schedule(PreUpdate);
        let messages = app
            .world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M, TestTimeline>>()
            .unwrap()
            .receive()
            .collect::<Vec<_>>();
        assert_eq!(messages, vec![M(1), M(2)]);
    }

    #[test]
    fn direction_adds_senders_but_not_receivers() {
        let mut app = message_test_app(true);
        let client = app.world_mut().spawn(Client).id();
        app.world_mut().flush();

        let client = app.world().entity(client);
        assert!(client.contains::<MessageSender<M>>());
        assert!(client.contains::<EventSender<E>>());
        assert!(!client.contains::<MessageReceiver<M, TestTimeline>>());
        assert!(!client.contains::<PendingTimelineEvents<E, TestTimeline>>());
    }

    #[test]
    fn timeline_event_waits_and_then_triggers() {
        let mut app = message_test_app(true);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(6);
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<TimelineChannel>(registry);
        transport.add_receiver_from_registry::<TimelineChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                EventSender::<E>::default(),
                TestTimeline(Tick(2).into()),
                RemoteId(PeerId::Local(0)),
                Linked,
                Connected,
            ))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<EventSender<E>>()
            .unwrap()
            .trigger::<TimelineChannel>(E(1));
        app.update();
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(app.world().resource::<EventCount>().0, 0);

        app.world_mut()
            .entity_mut(entity)
            .get_mut::<TestTimeline>()
            .unwrap()
            .set_now(Tick(6).into());
        app.world_mut().run_schedule(PreUpdate);
        app.world_mut().flush();
        assert_eq!(app.world().resource::<EventCount>().0, 1);
    }

    #[test]
    fn host_client_timeline_event_waits_and_then_triggers() {
        let mut app = message_test_app(true);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(6);
        let entity = app
            .world_mut()
            .spawn((
                EventSender::<E>::default(),
                TestTimeline(Tick(2).into()),
                HostClient { buffer: Vec::new() },
                RemoteId(PeerId::Local(0)),
                Connected,
            ))
            .id();
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<EventSender<E>>()
            .unwrap()
            .trigger::<TimelineChannel>(E(1));

        app.update();
        assert_eq!(app.world().resource::<EventCount>().0, 0);

        app.world_mut()
            .entity_mut(entity)
            .get_mut::<TestTimeline>()
            .unwrap()
            .set_now(Tick(6).into());
        app.world_mut().run_schedule(PreUpdate);
        app.world_mut().flush();
        assert_eq!(app.world().resource::<EventCount>().0, 1);
    }

    #[test]
    fn each_connection_uses_its_own_timeline() {
        let mut app = message_test_app(false);
        let first = app
            .world_mut()
            .spawn((
                MessageReceiver::<M, TestTimeline>::default(),
                TestTimeline(Tick(4).into()),
                RemoteId(PeerId::Local(1)),
                Connected,
            ))
            .id();
        let second = app
            .world_mut()
            .spawn((
                MessageReceiver::<M, TestTimeline>::default(),
                TestTimeline(Tick(9).into()),
                RemoteId(PeerId::Local(2)),
                Connected,
            ))
            .id();
        for (value, entity) in [first, second].into_iter().enumerate() {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<MessageReceiver<M, TestTimeline>>()
                .unwrap()
                .push_received(
                    M(value),
                    Tick(7),
                    ChannelKind::of::<TestChannel>(),
                    None,
                    &TimelineMessageConfig::default(),
                )
                .unwrap();
        }

        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world()
                .entity(first)
                .get::<MessageReceiver<M, TestTimeline>>()
                .unwrap()
                .num_messages(),
            0
        );
        assert_eq!(
            app.world()
                .entity(second)
                .get::<MessageReceiver<M, TestTimeline>>()
                .unwrap()
                .num_messages(),
            1
        );
    }

    #[test]
    fn disconnect_clears_pending_messages_and_events() {
        let mut app = message_test_app(true);
        let entity = app
            .world_mut()
            .spawn((
                MessageReceiver::<M, TestTimeline>::default(),
                PendingTimelineEvents::<E, TestTimeline>::default(),
                TestTimeline(Tick(1).into()),
                RemoteId(PeerId::Local(1)),
                Connected,
            ))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M, TestTimeline>>()
            .unwrap()
            .push_received(
                M(1),
                Tick(20),
                ChannelKind::of::<TestChannel>(),
                None,
                &TimelineMessageConfig::default(),
            )
            .unwrap();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<PendingTimelineEvents<E, TestTimeline>>()
            .unwrap()
            .push(
                E(1),
                PeerId::Local(1),
                Tick(20),
                ChannelKind::of::<TestChannel>(),
                None,
                &TimelineMessageConfig::default(),
            )
            .unwrap();
        app.world_mut()
            .entity_mut(entity)
            .insert(Disconnected::default());

        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world()
                .entity(entity)
                .get::<MessageReceiver<M, TestTimeline>>()
                .unwrap()
                .num_pending_timeline_messages(),
            0
        );
        assert_eq!(
            app.world()
                .entity(entity)
                .get::<PendingTimelineEvents<E, TestTimeline>>()
                .unwrap()
                .num_pending(),
            0
        );
    }
}
