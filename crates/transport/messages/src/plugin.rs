use crate::MessageManager;
use crate::registry::{MessageModeMetadata, MessageRegistry};
use bevy_app::{App, Last, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::{Add, On, With};
use bevy_ecs::{
    schedule::{IntoScheduleConfigs, SystemSet},
    system::{ParallelCommands, ParamBuilder, Query, QueryParamBuilder, Res, SystemParamBuilder},
    world::{DeferredWorld, FilteredEntityMut, World},
};
use lightyear_connection::client::{Connected, Disconnected};
use lightyear_core::prelude::{TimelineRegistry, TimelineSystems};
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

/// Maximum number of delayed payloads buffered per payload type and timeline.
pub const MAX_PENDING_TIMELINE_PAYLOADS: usize = 4096;

/// Maximum number of ticks a delivery timeline may lag behind a payload target.
pub const MAX_TIMELINE_LAG_TICKS: u32 = 100;

impl MessagePlugin {
    /// Resets receive state when a connection ends.
    ///
    /// Typed message receivers and private timeline-event queues are recreated
    /// lazily, so removing them is enough to discard both ready and pending
    /// payloads from the previous connection epoch.
    fn handle_disconnection(trigger: On<Add, Disconnected>, mut world: DeferredWorld) {
        let receiver_components = {
            let Some(registry) = world.get_resource::<MessageRegistry>() else {
                return;
            };
            registry
                .metadata
                .values()
                .map(|metadata| metadata.receive_component_id())
                .collect::<alloc::vec::Vec<_>>()
        };

        let entity = trigger.entity;
        world.commands().queue(move |world: &mut World| {
            let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                return;
            };
            if let Some(mut manager) = entity_mut.get_mut::<MessageManager>() {
                manager.entity_mapper.clear();
                manager.receive_messages.clear();
            }
            for component_id in receiver_components {
                entity_mut.remove_by_id(component_id);
            }
        });
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
        timeline_registry: Res<TimelineRegistry>,
        commands: ParallelCommands,
    ) {
        for (manager, mut entity) in entities.iter_mut() {
            for (timeline_kind, timeline_metadata) in timeline_registry.iter() {
                let Some(timeline) = entity.get_by_id(timeline_metadata.component_id()) else {
                    continue;
                };
                // SAFETY: the metadata is registered together with this component id.
                let tick = unsafe { timeline_metadata.tick(timeline) };

                for (kind, component_id) in &manager.receive_messages {
                    if let Some(receiver) = entity.get_mut_by_id(*component_id) {
                        let Some(metadata) = registry.metadata.get(kind) else {
                            continue;
                        };
                        let MessageModeMetadata::Message {
                            receive: metadata, ..
                        } = &metadata.mode
                        else {
                            continue;
                        };
                        // SAFETY: the callback is registered for this receiver component id.
                        unsafe { (metadata.release_timeline_fn)(receiver, *timeline_kind, tick) };
                    }
                }

                for metadata in
                    registry
                        .metadata
                        .values()
                        .filter_map(|metadata| match &metadata.mode {
                            MessageModeMetadata::Trigger { receive, .. } => Some(receive),
                            MessageModeMetadata::Message { .. } => None,
                        })
                {
                    if let Some(receiver) = entity.get_mut_by_id(metadata.component_id) {
                        // SAFETY: the callback is registered for this receiver component id.
                        unsafe { (metadata.release_fn)(receiver, &commands, *timeline_kind, tick) };
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
        app.init_resource::<TimelineRegistry>();

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
        let timeline_registry = app.world().resource::<TimelineRegistry>().clone();

        let recv = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.receive_component_id());
                    });
                    timeline_registry.values().for_each(|metadata| {
                        b.ref_id(metadata.component_id());
                    });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
            // RepliconMapParam: replicon's shared entity map when the feature is on
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::recv)
            .with_name("MessagePlugin::recv");

        let clear = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry
                        .metadata
                        .values()
                        .filter_map(|metadata| match &metadata.mode {
                            MessageModeMetadata::Message { receive, .. } => Some(receive),
                            MessageModeMetadata::Trigger { .. } => None,
                        })
                        .for_each(|receive| {
                            b.mut_id(receive.component_id);
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
                    registry.metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.send_component_id());
                    });
                });
            }),
            ParamBuilder,
            ParamBuilder,
            // RepliconMapParam: replicon's shared entity map when the feature is on
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
                    registry.metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.send_component_id());
                        b.mut_id(metadata.receive_component_id());
                    });
                    timeline_registry.values().for_each(|metadata| {
                        b.ref_id(metadata.component_id());
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
                    registry.metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.receive_component_id());
                    });
                    timeline_registry.values().for_each(|metadata| {
                        b.ref_id(metadata.component_id());
                    });
                });
            }),
            ParamBuilder,
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
    use crate::receive::MessageReceiver;
    use crate::receive_event::{PendingTimelineEvents, RemoteEvent};
    use crate::registry::AppMessageExt;
    use crate::send::MessageSender;
    use crate::send_trigger::EventSender;
    use crate::trigger::AppTriggerExt;
    use alloc::{vec, vec::Vec};
    use bevy_ecs::event::Event;
    use bevy_ecs::prelude::{Component, Entity, ResMut, Resource};
    #[cfg(feature = "client")]
    use lightyear_connection::client::Client;
    use lightyear_connection::client::Connected;
    use lightyear_connection::direction::NetworkDirection;
    use lightyear_connection::host::HostClient;
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_core::plugin::CorePlugins;
    use lightyear_core::prelude::{LocalTimeline, NetworkTimeline, Tick, TimelineKind};
    use lightyear_core::time::{Overstep, TickDelta, TickInstant};
    use lightyear_core::timeline::TimelineConfig;
    use lightyear_link::{Link, Linked, recv_payload_from_bytes};
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

    #[derive(Resource, Default)]
    struct EventCount(usize);

    #[derive(Resource, Default)]
    struct ObservedAfterReceive(bool);

    fn count_event(_: On<RemoteEvent<E>>, mut count: ResMut<EventCount>) {
        count.0 += 1;
    }

    fn observe_after_receive(
        receivers: Query<&MessageReceiver<M>>,
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
            ChannelSettings::default().on_timeline::<TestTimeline>(),
        );
        app.init_resource::<lightyear_connection::network_topology::NetworkingMetadata>();
        app.register_message::<M>()
            .add_direction(NetworkDirection::Bidirectional);
        if register_event {
            app.register_event::<E>()
                .add_direction(NetworkDirection::Bidirectional);
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
        link.recv.push_raw(recv_payload_from_bytes(payload));
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
        transport.add_channel_send_from_registry::<TestChannel>(registry);
        transport.add_channel_receive_from_registry::<TestChannel>(registry);
        // TODO: are these tests useful? they need so many components from other plugins..
        let mut entity_mut = app.world_mut().spawn((
            Link::default(),
            transport,
            MessageSender::<M>::default(),
            RemoteId(PeerId(1)),
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
        link.recv.push_raw(recv_payload_from_bytes(payload));

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
            RemoteId(PeerId(1)),
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
                None,
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
        transport.add_channel_send_from_registry::<TimelineChannel>(registry);
        transport.add_channel_receive_from_registry::<TimelineChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                MessageSender::<M>::default(),
                TestTimeline(Tick(5).into()),
                RemoteId(PeerId(1)),
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
            .get::<MessageReceiver<M>>()
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
            .get_mut::<MessageReceiver<M>>()
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
        transport.add_channel_send_from_registry::<TimelineChannel>(registry);
        transport.add_channel_receive_from_registry::<TimelineChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                MessageSender::<M>::default(),
                TestTimeline(Tick(10).into()),
                RemoteId(PeerId(1)),
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
            .get_mut::<MessageReceiver<M>>()
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
        transport.add_channel_send_from_registry::<TestChannel>(registry);
        transport.add_channel_receive_from_registry::<TestChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                MessageSender::<M>::default(),
                TestTimeline(Tick(1).into()),
                RemoteId(PeerId(1)),
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
        transport.add_channel_send_from_registry::<TimelineChannel>(registry);
        transport.add_channel_receive_from_registry::<TimelineChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                MessageSender::<M>::default(),
                RemoteId(PeerId(1)),
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

        assert!(!app.world().entity(entity).contains::<MessageReceiver<M>>());
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
                RemoteId(PeerId(1)),
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
            .get::<MessageReceiver<M>>()
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
                .get_mut::<MessageReceiver<M>>()
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
                RemoteId(PeerId(1)),
                Connected,
            ))
            .id();
        app.world_mut().flush();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TimelineChannel>(M(11));

        assert!(!app.world().entity(entity).contains::<MessageReceiver<M>>());

        app.update();

        let receiver = app
            .world()
            .entity(entity)
            .get::<MessageReceiver<M>>()
            .unwrap();
        assert_eq!(receiver.num_pending_timeline_messages(), 1);
    }

    #[test]
    fn host_client_drops_queued_messages_after_a_timeline_error() {
        let mut app = message_test_app(false);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta((MAX_TIMELINE_LAG_TICKS + 10) as i32);
        let entity = app
            .world_mut()
            .spawn((
                MessageSender::<M>::default(),
                TestTimeline(Tick(0).into()),
                HostClient { buffer: Vec::new() },
                RemoteId(PeerId(1)),
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
        assert!(!app.world().entity(entity).contains::<MessageReceiver<M>>());

        let local_tick = app.world().resource::<LocalTimeline>().tick();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<TestTimeline>()
            .unwrap()
            .set_now(local_tick.into());
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TimelineChannel>(M(3));
        app.update();
        app.world_mut().run_schedule(PreUpdate);
        let messages = app
            .world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M>>()
            .unwrap()
            .receive()
            .collect::<Vec<_>>();
        assert_eq!(messages, vec![M(3)]);
    }

    // Requires the `client` feature: without it `add_direction` is a no-op and
    // no senders are registered, so this test would fail. (The `Client`
    // component type itself is always compiled, which is why this fails at
    // runtime rather than at compile time. It passes in CI only because the
    // multi-package invocation unifies the facade's features onto this crate.)
    #[test]
    #[cfg(feature = "client")]
    fn direction_adds_senders_but_not_receivers() {
        let mut app = message_test_app(true);
        let client = app.world_mut().spawn(Client).id();
        app.world_mut().flush();

        let client = app.world().entity(client);
        assert!(client.contains::<MessageSender<M>>());
        assert!(client.contains::<EventSender<E>>());
        assert!(!client.contains::<MessageReceiver<M>>());
        assert!(!client.contains::<PendingTimelineEvents<E>>());
    }

    #[test]
    fn remote_event_error_does_not_requeue_events() {
        let mut app = message_test_app(true);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                Transport::default(),
                EventSender::<E>::default(),
                RemoteId(PeerId(1)),
                Linked,
                Connected,
            ))
            .id();
        {
            let mut entity_mut = app.world_mut().entity_mut(entity);
            let mut sender = entity_mut.get_mut::<EventSender<E>>().unwrap();
            sender.trigger::<TestChannel>(E(1));
            sender.trigger::<TestChannel>(E(2));
        }

        // Sending fails because the transport has no sender for `TestChannel`.
        app.update();

        let transport = {
            let registry = app.world().resource::<ChannelRegistry>();
            let mut transport = Transport::default();
            transport.add_channel_send_from_registry::<TestChannel>(registry);
            transport.add_channel_receive_from_registry::<TestChannel>(registry);
            transport
        };
        app.world_mut().entity_mut(entity).insert(transport);
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<EventSender<E>>()
            .unwrap()
            .trigger::<TestChannel>(E(3));

        app.update();
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);
        app.world_mut().flush();

        assert_eq!(app.world().resource::<EventCount>().0, 1);
    }

    #[test]
    fn timeline_event_waits_and_then_triggers() {
        let mut app = message_test_app(true);
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(6);
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_channel_send_from_registry::<TimelineChannel>(registry);
        transport.add_channel_receive_from_registry::<TimelineChannel>(registry);
        let entity = app
            .world_mut()
            .spawn((
                Link::default(),
                transport,
                EventSender::<E>::default(),
                TestTimeline(Tick(2).into()),
                RemoteId(PeerId(1)),
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
        app.add_observer(|event: On<RemoteEvent<E>>| {
            assert_eq!(event.from, PeerId(73));
        });
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(6);
        let entity = app
            .world_mut()
            .spawn((
                EventSender::<E>::default(),
                TestTimeline(Tick(2).into()),
                HostClient { buffer: Vec::new() },
                RemoteId(PeerId(73)),
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
                MessageReceiver::<M>::default(),
                TestTimeline(Tick(4).into()),
                RemoteId(PeerId(1)),
                Connected,
            ))
            .id();
        let second = app
            .world_mut()
            .spawn((
                MessageReceiver::<M>::default(),
                TestTimeline(Tick(9).into()),
                RemoteId(PeerId(2)),
                Connected,
            ))
            .id();
        for (value, entity) in [first, second].into_iter().enumerate() {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<MessageReceiver<M>>()
                .unwrap()
                .push_received(
                    M(value),
                    Tick(7),
                    ChannelKind::of::<TestChannel>(),
                    None,
                    Some(TimelineKind::of::<TestTimeline>()),
                )
                .unwrap();
        }

        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world()
                .entity(first)
                .get::<MessageReceiver<M>>()
                .unwrap()
                .num_messages(),
            0
        );
        assert_eq!(
            app.world()
                .entity(second)
                .get::<MessageReceiver<M>>()
                .unwrap()
                .num_messages(),
            1
        );
    }

    #[test]
    fn disconnect_removes_message_and_event_receivers() {
        let mut app = message_test_app(true);
        let entity = app
            .world_mut()
            .spawn((
                MessageReceiver::<M>::default(),
                PendingTimelineEvents::<E>::default(),
                TestTimeline(Tick(1).into()),
                RemoteId(PeerId(1)),
                Connected,
            ))
            .id();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M>>()
            .unwrap()
            .push_received(M(0), Tick(1), ChannelKind::of::<TestChannel>(), None, None)
            .unwrap();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M>>()
            .unwrap()
            .push_received(
                M(1),
                Tick(20),
                ChannelKind::of::<TestChannel>(),
                None,
                Some(TimelineKind::of::<TestTimeline>()),
            )
            .unwrap();
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<PendingTimelineEvents<E>>()
            .unwrap()
            .push(
                E(1),
                PeerId(1),
                Tick(20),
                ChannelKind::of::<TestChannel>(),
                None,
                TimelineKind::of::<TestTimeline>(),
            )
            .unwrap();
        app.world_mut()
            .entity_mut(entity)
            .insert(Disconnected::default());

        app.world_mut().run_schedule(PreUpdate);
        let entity_ref = app.world().entity(entity);
        assert!(!entity_ref.contains::<MessageReceiver<M>>());
        assert!(!entity_ref.contains::<PendingTimelineEvents<E>>());
        assert!(
            entity_ref
                .get::<MessageManager>()
                .unwrap()
                .receive_messages
                .is_empty()
        );
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Gameplay(u8);

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct ControlSnapshot(Vec<u8>);

    struct OrderedControlChannel;

    fn forbidden_deserialize<'de, D: serde::Deserializer<'de>>(_: D) -> Result<u8, D::Error> {
        panic!("preadmission payload reached typed deserialization");
    }

    #[derive(Event, Serialize, Deserialize)]
    struct Forbidden(#[serde(deserialize_with = "forbidden_deserialize")] u8);

    fn preadmission_app(event: bool) -> App {
        let mut app = App::new();
        app.add_plugins(CorePlugins {
            tick_duration: core::time::Duration::from_millis(10),
        });
        app.add_plugins(TestTransportPlugin);
        app.add_channel::<OrderedControlChannel>(ChannelSettings {
            mode: lightyear_transport::prelude::ChannelMode::OrderedReliable(Default::default()),
            ..Default::default()
        });
        app.add_channel::<TimelineChannel>(
            ChannelSettings::default().on_timeline::<TestTimeline>(),
        );
        app.init_resource::<lightyear_connection::network_topology::NetworkingMetadata>();
        app.register_message::<M>();
        app.register_message::<Gameplay>();
        app.register_message::<ControlSnapshot>();
        app.register_event::<E>();
        if event {
            app.register_event::<Forbidden>();
        } else {
            app.register_message::<Forbidden>();
        }
        app.init_resource::<EventCount>();
        app.add_observer(count_event);
        app.add_plugins(MessagePlugin);
        app.finish();
        app
    }

    fn pending_link(app: &mut App, policy: crate::PreadmissionMessage) -> Entity {
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_channel_send_from_registry::<TestChannel>(registry);
        transport.add_channel_receive_from_registry::<TestChannel>(registry);
        transport.add_channel_send_from_registry::<TimelineChannel>(registry);
        transport.add_channel_receive_from_registry::<TimelineChannel>(registry);
        transport.add_channel_send_from_registry::<OrderedControlChannel>(registry);
        transport.add_channel_receive_from_registry::<OrderedControlChannel>(registry);
        app.world_mut()
            .spawn((
                Link::default(),
                Linked,
                transport,
                MessageManager::default(),
                policy,
            ))
            .id()
    }

    fn queue_message<M: crate::Message, C: lightyear_transport::channel::Channel>(
        app: &mut App,
        entity: Entity,
        message: &M,
    ) {
        let mut writer = lightyear_serde::writer::Writer::default();
        let local = lightyear_serde::entity_map::SendEntityMap::default();
        app.world()
            .resource::<MessageRegistry>()
            .serialize(
                message,
                &mut writer,
                &lightyear_serde::entity_map::SendMapView {
                    shared: None,
                    local: &local,
                },
            )
            .unwrap();
        app.world()
            .get::<Transport>(entity)
            .unwrap()
            .send_erased(ChannelKind::of::<C>(), writer.take_written(), 1.0)
            .unwrap();
    }

    fn inject_message<M: crate::Message, C: lightyear_transport::channel::Channel>(
        app: &mut App,
        entity: Entity,
        message: &M,
    ) {
        queue_message::<M, C>(app, entity, message);
        app.world_mut().run_schedule(PostUpdate);
        loopback_link_payload(app, entity);
        app.world_mut().run_schedule(PreUpdate);
    }

    #[test]
    fn preadmission_delivers_control_without_identity_and_preserves_other_sender_queues() {
        let mut app = preadmission_app(false);
        let entity = pending_link(&mut app, crate::PreadmissionMessage::new::<M>(64));
        let mut control = MessageSender::<M>::default();
        control.send::<TestChannel>(M(7));
        let mut gameplay = MessageSender::<Gameplay>::default();
        gameplay.send::<TestChannel>(Gameplay(9));
        let mut event = EventSender::<E>::default();
        event.trigger::<TestChannel>(E(11));
        app.world_mut()
            .entity_mut(entity)
            .insert((control, gameplay, event));
        app.world_mut().run_schedule(PostUpdate);
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world_mut()
                .get_mut::<MessageReceiver<M>>(entity)
                .unwrap()
                .receive()
                .collect::<Vec<_>>(),
            vec![M(7)],
        );
        let link = app.world().entity(entity);
        assert!(!link.contains::<Connected>());
        assert!(!link.contains::<RemoteId>());
        assert!(!link.contains::<lightyear_core::id::LocalId>());
        assert!(!link.contains::<MessageReceiver<Gameplay>>());
        assert_eq!(app.world().resource::<EventCount>().0, 0);

        app.world_mut()
            .entity_mut(entity)
            .insert((RemoteId(PeerId(1)), Connected));
        app.world_mut().run_schedule(PostUpdate);
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world_mut()
                .get_mut::<MessageReceiver<Gameplay>>(entity)
                .unwrap()
                .receive()
                .collect::<Vec<_>>(),
            vec![Gameplay(9)],
        );
        assert_eq!(app.world().resource::<EventCount>().0, 1);
    }

    #[test]
    fn preadmission_rejects_unrelated_types_and_events_before_deserialization() {
        for event in [false, true] {
            let mut app = preadmission_app(event);
            let entity = pending_link(&mut app, crate::PreadmissionMessage::new::<M>(64));
            inject_message::<_, TestChannel>(&mut app, entity, &Forbidden(3));
            assert!(
                !app.world()
                    .entity(entity)
                    .contains::<MessageReceiver<Forbidden>>()
            );
            assert!(
                !app.world()
                    .entity(entity)
                    .contains::<PendingTimelineEvents<Forbidden>>()
            );
            if event {
                // Even naming an event as the policy's type must not enable event decoding.
                app.world_mut()
                    .entity_mut(entity)
                    .insert(crate::PreadmissionMessage::new::<Forbidden>(64));
                inject_message::<_, TestChannel>(&mut app, entity, &Forbidden(4));
            }
        }
    }

    #[test]
    fn preadmission_rejects_timeline_and_oversized_input_before_deserialization() {
        let mut app = preadmission_app(false);
        let entity = pending_link(&mut app, crate::PreadmissionMessage::new::<Forbidden>(64));
        inject_message::<_, TimelineChannel>(&mut app, entity, &Forbidden(3));
        assert!(
            !app.world()
                .entity(entity)
                .contains::<MessageReceiver<Forbidden>>()
        );
        app.world_mut()
            .entity_mut(entity)
            .insert(crate::PreadmissionMessage::new::<Forbidden>(0));
        inject_message::<_, TestChannel>(&mut app, entity, &Forbidden(3));
        assert!(
            !app.world()
                .entity(entity)
                .contains::<MessageReceiver<Forbidden>>()
        );
    }

    #[test]
    fn preadmission_rejects_oversized_output_and_timeline_sends() {
        let mut app = preadmission_app(false);
        let entity = pending_link(&mut app, crate::PreadmissionMessage::new::<M>(1));
        let mut control = MessageSender::<M>::default();
        control.send::<TestChannel>(M(1024));
        app.world_mut().entity_mut(entity).insert(control);
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(app.world().get::<Link>(entity).unwrap().send.len(), 0);

        app.world_mut()
            .entity_mut(entity)
            .insert(crate::PreadmissionMessage::new::<M>(64));
        app.world_mut()
            .get_mut::<MessageSender<M>>(entity)
            .unwrap()
            .send::<TimelineChannel>(M(1));
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(app.world().get::<Link>(entity).unwrap().send.len(), 0);
        // Rejection must not contaminate the next serialized control message.
        app.world_mut()
            .get_mut::<MessageSender<M>>(entity)
            .unwrap()
            .send::<TestChannel>(M(2));
        app.world_mut().run_schedule(PostUpdate);
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world_mut()
                .get_mut::<MessageReceiver<M>>(entity)
                .unwrap()
                .receive()
                .collect::<Vec<_>>(),
            vec![M(2)],
        );
    }

    #[test]
    fn preadmission_requires_policy_linked_and_no_disconnection() {
        for disabled in 0..4 {
            let mut app = preadmission_app(false);
            let entity = pending_link(&mut app, crate::PreadmissionMessage::new::<M>(64));
            let mut control = MessageSender::<M>::default();
            control.send::<TestChannel>(M(1));
            app.world_mut().entity_mut(entity).insert(control);
            // Queue incoming bytes while the path is enabled, but do not decode yet.
            app.world_mut().run_schedule(PostUpdate);
            loopback_link_payload(&mut app, entity);
            match disabled {
                0 => {
                    app.world_mut()
                        .entity_mut(entity)
                        .remove::<crate::PreadmissionMessage>();
                }
                1 => {
                    app.world_mut().entity_mut(entity).remove::<Linked>();
                }
                2 => {
                    app.world_mut()
                        .entity_mut(entity)
                        .insert(Disconnected::default());
                }
                _ => {
                    app.world_mut()
                        .entity_mut(entity)
                        .insert(lightyear_connection::client::Disconnecting);
                }
            }
            app.world_mut()
                .get_mut::<MessageSender<M>>(entity)
                .unwrap()
                .send::<TestChannel>(M(2));
            app.world_mut().run_schedule(PreUpdate);
            app.world_mut().run_schedule(PostUpdate);
            assert!(!app.world().entity(entity).contains::<MessageReceiver<M>>());
            // Transport acknowledgements may still be emitted. Re-enable receive without running
            // send: the control queued while disabled must not already be on the wire.
            {
                let mut link = app.world_mut().get_mut::<Link>(entity).unwrap();
                while let Some(payload) = link.send.pop() {
                    link.recv.push_raw(recv_payload_from_bytes(payload));
                }
            }
            app.world_mut()
                .entity_mut(entity)
                .remove::<(Disconnected, lightyear_connection::client::Disconnecting)>()
                .insert((Linked, crate::PreadmissionMessage::new::<M>(64)));
            for _ in 0..2 {
                app.world_mut().run_schedule(PreUpdate);
                if let Some(mut receiver) = app.world_mut().get_mut::<MessageReceiver<M>>(entity) {
                    assert!(!receiver.receive().any(|message| message == M(2)));
                }
            }
        }
    }

    #[test]
    fn preadmission_retains_later_ordered_snapshot_until_admission() {
        let mut app = preadmission_app(false);
        let entity = pending_link(
            &mut app,
            crate::PreadmissionMessage::new::<ControlSnapshot>(16),
        );
        // A remote may already have admitted us and send its full snapshot directly
        // after its own small identity announcement, even in the same packet.
        queue_message::<_, OrderedControlChannel>(&mut app, entity, &ControlSnapshot(vec![1]));
        queue_message::<_, OrderedControlChannel>(&mut app, entity, &ControlSnapshot(vec![2; 64]));
        app.world_mut().run_schedule(PostUpdate);
        loopback_link_payload(&mut app, entity);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world_mut()
                .get_mut::<MessageReceiver<ControlSnapshot>>(entity)
                .unwrap()
                .receive()
                .collect::<Vec<_>>(),
            vec![ControlSnapshot(vec![1])],
        );

        // Admission occurs after the receive phase and lifts the preadmission cap.
        app.world_mut()
            .entity_mut(entity)
            .insert((RemoteId(PeerId(1)), Connected))
            .remove::<crate::PreadmissionMessage>();
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world_mut()
                .get_mut::<MessageReceiver<ControlSnapshot>>(entity)
                .unwrap()
                .receive()
                .collect::<Vec<_>>(),
            vec![ControlSnapshot(vec![2; 64])],
        );
    }
}
