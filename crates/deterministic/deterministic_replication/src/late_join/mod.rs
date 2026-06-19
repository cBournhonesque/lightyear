//! Late-join catch-up for deterministic replication.
//!
//! # Problem
//!
//! In deterministic replication the server and all clients run the same
//! simulation driven only by inputs. For an already-connected client, the
//! initial state of an entity is established once (via `replicate_once` of
//! physics components at entity spawn) and then every peer simulates forward
//! from that point using inputs that are rebroadcast by the server.
//!
//! When a new client joins mid-game, that initial snapshot is already in the
//! past. Simply `replicate_once`-ing the *current* physics state would not
//! help because the new client does not yet have the remote inputs needed
//! to simulate forward from the snapshot tick.
//!
//! # Approach (client-driven, bundled)
//!
//! Information flows from the client — which actually knows what it has —
//! back to the server:
//!
//! 1. At join, the server replicates "structural" data for existing entities
//!    (markers like `PlayerId`, `DeterministicPredicted`) and starts
//!    rebroadcasting inputs for those entities. Physics components
//!    registered via [`AppCatchUpExt::register_catchup`] are
//!    **hidden by default** via a replicon per-component visibility filter
//!    until each client requests a catch-up snapshot.
//!
//! 2. On the client, replicated [`CatchUpGated`] entities are automatically
//!    marked with [`AwaitingCatchUpSnapshot`]. Once the [`InputTimeline`] is
//!    synced and registered input buffers cover a replay-safe tick, the plugin
//!    sends [`CatchUpRequest`] with that `input_safe_tick`.
//!
//! 3. On the server, the [`CatchUpRequest`] handler accepts the request at the
//!    server's current authoritative tick. It then inserts [`HasCaughtUp`] on
//!    the client's link entity and sends a replicated [`CatchUpSnapshotReady`]
//!    event with the authoritative rollback tick and the Replicon checkpoint
//!    that contains the reveal.
//!
//! 4. The client waits until Replicon's mutate completion state reports that
//!    the accepted Replicon checkpoint is fully confirmed. At that point all
//!    mutation messages for the catch-up reveal have landed, so the plugin
//!    emits [`CatchUpSnapshotReady`] for local-only setup and schedules one
//!    forced rollback to the accepted server tick.
//!
//! 5. [`HasCaughtUp`] is not removed. After the initial catch-up, the rest of
//!    the simulation is deterministic, so later catch-up-gated components are
//!    replicated normally to that client.
//!
//! # Why bundled, not per-entity
//!
//! A per-entity catch-up (one [`CatchUpRequest`] per gated entity, each
//! producing its own snapshot at its own tick) causes divergence. The first
//! forced rollback replays every entity, but entities that neither have a
//! snapshot nor valid local history at that tick are replayed from stale local
//! state. Bundling gives the plugin one coherent set of pending entities and
//! one accepted replication checkpoint to confirm before rollback.
//!
//! # Why per-component visibility, not entity-level
//!
//! If we hid the whole entity, the client would not know the entity
//! exists and could not send the catch-up request (nor receive rebroadcast
//! inputs). By keeping the entity visible but hiding only the physics
//! components, the client sees the entity + marker components + rebroadcast
//! inputs and can request the bundled snapshot.

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;
mod shared;

#[cfg(feature = "client")]
pub use client::{CatchUpClientTimeout, CatchUpManager};
pub use shared::{
    AppCatchUpExt, AwaitingCatchUpSnapshot, CatchUpComponentScope, CatchUpGated, CatchUpRegistry,
    CatchUpRequest, CatchUpSnapshotReady, CatchUpSystems, HasCaughtUp,
};

#[cfg(all(test, feature = "client"))]
pub(crate) use client::{
    PendingCatchUpSnapshot, detect_catch_up_snapshot_ready, panic_if_catchup_request_stalled,
    update_client_catchup_input_readiness,
};
#[cfg(all(test, feature = "server"))]
pub(crate) use server::CatchUpVisibility;

use bevy_app::{App, Plugin};
use bevy_replicon::shared::{AuthMethod, RepliconSharedPlugin};
use lightyear_connection::direction::NetworkDirection;
use lightyear_inputs::input_message::ActionStateSequence;
use lightyear_messages::prelude::{AppMessageExt, AppTriggerExt};
use lightyear_replication::metadata::MetadataChannel;
use lightyear_replication::registry::replication::AppComponentExt;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, ReliableSettings};

use crate::mode::CatchUpMode;

/// Plugin that wires up the late-join catch-up machinery.
///
/// Clients send [`CatchUpRequest`] once they're synced, have at least one
/// catch-up-gated entity awaiting a snapshot, and have registered input
/// buffers covering a safe replay tick. The server accepts by inserting
/// [`HasCaughtUp`] and sending a replicated [`CatchUpSnapshotReady`] event.
/// The client waits for the accepted Replicon checkpoint to be fully
/// confirmed, re-triggers [`CatchUpSnapshotReady`] locally for activation
/// observers, and then drives the forced rollback. The [`HasCaughtUp`] marker
/// is kept after success.
pub struct LateJoinCatchUpPlugin;

impl Default for LateJoinCatchUpPlugin {
    fn default() -> Self {
        Self
    }
}

impl AppCatchUpExt for App {
    fn register_catchup<T, S>(&mut self) -> &mut Self
    where
        T: CatchUpComponentScope + Send + Sync + 'static,
        S: ActionStateSequence,
    {
        #[cfg(feature = "server")]
        server::register_catchup::<T>(self);

        #[cfg(feature = "client")]
        client::register_catchup::<S>(self);

        self
    }
}

impl Plugin for LateJoinCatchUpPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<RepliconSharedPlugin>() {
            app.add_plugins(RepliconSharedPlugin {
                auth_method: AuthMethod::None,
            });
        }

        app.add_channel::<MetadataChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        if !app.is_message_registered::<CatchUpRequest>() {
            app.register_message::<CatchUpRequest>()
                .add_direction(NetworkDirection::ClientToServer);
        }
        app.register_event::<CatchUpSnapshotReady>()
            .add_direction(NetworkDirection::ServerToClient);
        app.init_resource::<CatchUpRegistry>();
        app.init_resource::<CatchUpMode>();
        app.register_component::<CatchUpGated>();

        #[cfg(feature = "client")]
        client::build(app);

        #[cfg(feature = "server")]
        server::build(app);
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the late-join catch-up registry and filter marker
    //! wiring. Replicon's own tests cover the private visibility mask state;
    //! these tests verify that `CatchUpGated` gets the registered filter
    //! component and that applying catch-up permanently marks the client with
    //! `HasCaughtUp`.
    use super::*;
    use alloc::vec::Vec;
    use bevy_app::{App, PreUpdate};
    use bevy_ecs::prelude::*;
    use bevy_replicon::prelude::{RepliconTick, SingleComponent};
    use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
    use bevy_replicon::server::visibility::registry::FilterRegistry;
    use bevy_replicon::shared::replication::registry::ReplicationRegistry;
    use core::time::Duration;
    use lightyear_connection::client::Client;
    use lightyear_connection::client_of::ClientOf;
    use lightyear_core::tick::Tick;
    use lightyear_inputs::input_buffer::InputBuffer;
    use lightyear_inputs::input_message::ActionStateSequence;
    use lightyear_messages::prelude::MessageSender;
    use lightyear_replication::checkpoint::ReplicationCheckpointMap;
    use lightyear_replication::prelude::ServerMutateTicks;
    use lightyear_sync::prelude::{InputTimeline, IsSynced};
    use serde::{Deserialize, Serialize};

    #[derive(Component, Default)]
    struct A;
    #[derive(Component, Default)]
    struct B;
    #[derive(Component, Default)]
    struct C;

    #[derive(Component, Default)]
    struct TestInputMarker;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
    struct TestAction;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
    struct TestSnapshot;

    impl lightyear_inputs::input_message::InputSnapshot for TestSnapshot {
        fn decay_tick(&mut self, _tick_duration: Duration) {}
    }

    #[derive(Component, Clone, Debug, Default, PartialEq)]
    struct TestState;

    impl lightyear_inputs::input_message::ActionStateQueryData for TestState {
        type Mut = &'static mut Self;
        type MutItemInner<'w> = &'w mut Self;
        type Main = Self;
        type Bundle = Self;

        fn as_read_only<'a, 'w: 'a, 's>(
            state: &'a <Self::Mut as bevy_ecs::query::QueryData>::Item<'w, 's>,
        ) -> <<Self::Mut as bevy_ecs::query::QueryData>::ReadOnly as bevy_ecs::query::QueryData>::Item<'a, 's>
        {
            state
        }

        fn into_inner<'w, 's>(
            mut_item: <Self::Mut as bevy_ecs::query::QueryData>::Item<'w, 's>,
        ) -> Self::MutItemInner<'w> {
            mut_item.into_inner()
        }

        fn as_mut(bundle: &mut Self::Bundle) -> Self::MutItemInner<'_> {
            bundle
        }

        fn base_value() -> Self::Bundle {
            Self
        }
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct TestSequence;

    impl ActionStateSequence for TestSequence {
        type Action = TestAction;
        type Snapshot = TestSnapshot;
        type State = TestState;
        type Marker = TestInputMarker;

        fn len(&self) -> usize {
            0
        }

        fn get_snapshots_from_message(
            self,
            _tick_duration: Duration,
        ) -> impl Iterator<Item = lightyear_inputs::input_buffer::Compressed<Self::Snapshot>>
        {
            core::iter::empty()
        }

        fn build_from_input_buffer(
            _input_buffer: &InputBuffer<Self::Snapshot, Self::Action>,
            _num_ticks: u32,
            _end_tick: Tick,
        ) -> Option<Self>
        where
            Self: Sized,
        {
            Some(Self)
        }

        fn to_snapshot(_state: &TestState) -> Self::Snapshot {
            TestSnapshot
        }

        fn from_snapshot(_state: &mut TestState, _snapshot: &Self::Snapshot) {}
    }

    #[derive(Resource, Default)]
    struct ReadyEvents(Vec<CatchUpSnapshotReady>);

    fn collect_ready_events(trigger: On<CatchUpSnapshotReady>, mut events: ResMut<ReadyEvents>) {
        events.0.push(trigger.event().clone());
    }

    /// Build an app with the full catch-up wiring: replicon filter
    /// registry + the plugin's registry and observer.
    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        app.register_catchup::<(A, B, C), TestSequence>();
        app
    }

    fn spawn_client(app: &mut App) -> Entity {
        // `Connected`'s on_insert hook asserts that the entity carries a
        // `RemoteId`, so the test client is shaped like a real remote-client
        // link entity.
        let server = app.world_mut().spawn_empty().id();
        app.world_mut()
            .spawn((
                ClientOf,
                ClientVisibility::default(),
                lightyear_link::server::LinkOf { server },
                lightyear_core::id::RemoteId(lightyear_core::id::PeerId::Server),
            ))
            .id()
    }

    #[test]
    fn register_is_idempotent() {
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        assert!(!app.world().resource::<CatchUpRegistry>().is_initialized());

        app.register_catchup::<(A, B, C), TestSequence>();
        assert!(app.world().resource::<CatchUpRegistry>().is_initialized());

        // Second call is a no-op and must not register the same filter twice.
        app.register_catchup::<(A, B, C), TestSequence>();
        assert!(app.world().resource::<CatchUpRegistry>().is_initialized());
    }

    #[test]
    fn register_with_single_component_still_works() {
        // `SingleComponent<C>` is a valid `FilterScope`, so this is the
        // fallback for users who only want to hide one component.
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.init_resource::<CatchUpRegistry>();
        app.register_catchup::<SingleComponent<A>, TestSequence>();
        assert!(app.world().resource::<CatchUpRegistry>().is_initialized());
    }

    #[test]
    fn catch_up_gated_does_not_panic_with_no_clients() {
        let mut app = test_app();
        let entity = app.world_mut().spawn(CatchUpGated).id();
        app.update();
        assert!(
            app.world()
                .get::<CatchUpVisibility<(A, B, C)>>(entity)
                .is_some()
        );
    }

    #[test]
    fn catch_up_gated_inserts_filter_with_clients() {
        let mut app = test_app();
        let _client_a = spawn_client(&mut app);
        let _client_b = spawn_client(&mut app);
        app.update();

        let entity = app.world_mut().spawn(CatchUpGated).id();
        // Observer runs via commands.queue; another update flushes.
        app.update();
        assert!(
            app.world()
                .get::<CatchUpVisibility<(A, B, C)>>(entity)
                .is_some()
        );
    }

    #[test]
    fn client_connecting_later_does_not_panic_with_existing_gated_entities() {
        let mut app = test_app();
        let _entity_one = app.world_mut().spawn(CatchUpGated).id();
        let _entity_two = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        let client = spawn_client(&mut app);
        app.world_mut()
            .entity_mut(client)
            .insert(lightyear_connection::client::Connected);
        app.update();
    }

    #[test]
    fn new_gated_entity_after_catch_up_gets_filter_without_removing_client_marker() {
        let mut app = test_app();
        let client = spawn_client(&mut app);
        let entity_one = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        assert!(
            app.world()
                .get::<CatchUpVisibility<(A, B, C)>>(entity_one)
                .is_some()
        );

        app.world_mut().entity_mut(client).insert(HasCaughtUp);
        assert!(app.world().get::<HasCaughtUp>(client).is_some());

        let entity_two = app.world_mut().spawn(CatchUpGated).id();
        app.update();

        assert!(
            app.world()
                .get::<CatchUpVisibility<(A, B, C)>>(entity_two)
                .is_some()
        );
        assert!(app.world().get::<HasCaughtUp>(client).is_some());
    }

    #[test]
    fn snapshot_ready_event_triggers_observer() {
        let mut app = App::new();
        app.init_resource::<CatchUpMode>();
        app.init_resource::<ServerMutateTicks>();
        app.init_resource::<ReplicationCheckpointMap>();
        app.init_resource::<ReadyEvents>();
        app.add_observer(collect_ready_events);
        app.add_systems(PreUpdate, detect_catch_up_snapshot_ready);

        let replicon_tick = RepliconTick::new(7);
        let server_tick = Tick(42);
        {
            let mut server_mutate_ticks = app.world_mut().resource_mut::<ServerMutateTicks>();
            assert!(server_mutate_ticks.confirm(replicon_tick, 1));
            let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
            checkpoints.record(replicon_tick, server_tick);
        }
        let mut manager = CatchUpManager::default();
        manager.request_sent_at_tick = Some(Tick(0));
        manager.request_input_safe_tick = Some(Tick(42));
        manager.pending_snapshot = Some(PendingCatchUpSnapshot {
            server_tick,
            replicon_tick,
        });
        app.world_mut().spawn((Client::default(), manager));
        app.world_mut().spawn(AwaitingCatchUpSnapshot);

        app.update();

        let events = &app.world().resource::<ReadyEvents>().0;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].replicon_tick, replicon_tick);
        assert_eq!(events[0].server_tick, server_tick);
    }

    #[test]
    fn snapshot_ready_event_requires_request_before_initial_catch_up() {
        let mut app = App::new();
        app.init_resource::<CatchUpMode>();
        app.init_resource::<ServerMutateTicks>();
        app.init_resource::<ReplicationCheckpointMap>();
        app.init_resource::<ReadyEvents>();
        app.add_observer(collect_ready_events);
        app.add_systems(PreUpdate, detect_catch_up_snapshot_ready);

        app.world_mut()
            .spawn((Client::default(), CatchUpManager::default()));
        app.world_mut().spawn(AwaitingCatchUpSnapshot);

        app.update();

        assert!(app.world().resource::<ReadyEvents>().0.is_empty());
    }

    #[test]
    fn completed_catchup_emits_event_from_global_confirmed_tick_for_new_gated_entities() {
        let mut app = App::new();
        app.init_resource::<CatchUpMode>();
        app.init_resource::<ServerMutateTicks>();
        app.init_resource::<ReplicationCheckpointMap>();
        app.init_resource::<ReadyEvents>();
        app.add_observer(collect_ready_events);
        app.add_systems(PreUpdate, detect_catch_up_snapshot_ready);

        let mut manager = CatchUpManager::default();
        manager.completed = true;
        app.world_mut().spawn((Client::default(), manager));
        let replicon_tick = RepliconTick::new(11);
        let server_tick = Tick(77);
        {
            let mut server_mutate_ticks = app.world_mut().resource_mut::<ServerMutateTicks>();
            assert!(server_mutate_ticks.confirm(replicon_tick, 1));
            let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
            checkpoints.record(replicon_tick, server_tick);
        }
        let _entity = app.world_mut().spawn(CatchUpGated).id();

        app.update();

        let events = &app.world().resource::<ReadyEvents>().0;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].replicon_tick, replicon_tick);
        assert_eq!(events[0].server_tick, server_tick);
    }

    #[test]
    fn catch_up_request_retries_same_input_safe_tick_before_acceptance() {
        let mut app = App::new();
        let mut timeline = lightyear_core::prelude::LocalTimeline::default();
        timeline.apply_delta(9);
        app.insert_resource(timeline);
        app.add_systems(
            PreUpdate,
            update_client_catchup_input_readiness::<TestSequence>,
        );

        let mut manager = CatchUpManager::default();
        manager.request_sent_at_tick = Some(Tick(0));
        manager.request_input_safe_tick = Some(Tick(5));
        app.world_mut().spawn((
            Client::default(),
            manager,
            MessageSender::<CatchUpRequest>::default(),
            IsSynced::<InputTimeline>::default(),
        ));
        app.world_mut().spawn(AwaitingCatchUpSnapshot);

        let mut input_buffer = InputBuffer::<TestSnapshot, TestAction>::default();
        input_buffer.set_empty(Tick(5));
        app.world_mut().spawn((input_buffer, TestInputMarker));

        app.update();

        let mut managers = app.world_mut().query::<&CatchUpManager>();
        let manager = managers.single(app.world()).unwrap();
        assert_eq!(manager.request_sent_at_tick, Some(Tick(9)));
        assert_eq!(manager.request_input_safe_tick, Some(Tick(5)));
    }

    #[test]
    #[should_panic(expected = "requested deterministic catch-up")]
    fn stalled_catch_up_request_panics_after_timeout() {
        let mut app = App::new();
        let mut timeline = lightyear_core::prelude::LocalTimeline::default();
        timeline.apply_delta(11);
        app.insert_resource(timeline);
        app.insert_resource(lightyear_core::tick::TickDuration(
            core::time::Duration::from_millis(10),
        ));
        app.insert_resource(CatchUpClientTimeout {
            duration: core::time::Duration::from_millis(100),
        });
        app.init_resource::<CatchUpMode>();
        let mut manager = CatchUpManager::default();
        manager.request_sent_at_tick = Some(Tick(0));
        manager.request_input_safe_tick = Some(Tick(0));
        manager.pending_snapshot = Some(PendingCatchUpSnapshot {
            server_tick: Tick(0),
            replicon_tick: RepliconTick::new(0),
        });
        app.world_mut().spawn((Client::default(), manager));
        app.world_mut().spawn(AwaitingCatchUpSnapshot);
        app.add_systems(PreUpdate, panic_if_catchup_request_stalled);

        app.update();
    }

    #[test]
    fn unaccepted_catch_up_request_does_not_panic_after_timeout() {
        let mut app = App::new();
        let mut timeline = lightyear_core::prelude::LocalTimeline::default();
        timeline.apply_delta(11);
        app.insert_resource(timeline);
        app.insert_resource(lightyear_core::tick::TickDuration(
            core::time::Duration::from_millis(10),
        ));
        app.insert_resource(CatchUpClientTimeout {
            duration: core::time::Duration::from_millis(100),
        });
        app.init_resource::<CatchUpMode>();
        let mut manager = CatchUpManager::default();
        manager.request_sent_at_tick = Some(Tick(0));
        manager.request_input_safe_tick = Some(Tick(0));
        app.world_mut().spawn((Client::default(), manager));
        app.world_mut().spawn(AwaitingCatchUpSnapshot);
        app.add_systems(PreUpdate, panic_if_catchup_request_stalled);

        app.update();
    }
}
