//! Stepper specialised for deterministic-replication tests.
//!
//! Differs from `ClientServerStepper` in that it does NOT auto-add the
//! generic `ProtocolPlugin`. The caller is expected to install
//! [`DetProtocolPlugin`] explicitly so the apps only have the exact
//! deterministic-replication surface area we want to exercise.

use crate::client_server::deterministic::protocol::DetProtocolPlugin;
use avian2d::prelude::*;
use bevy::MinimalPlugins;
use bevy::app::PluginsState;
use bevy::ecs::schedule::SingleThreadedExecutor;
use bevy::input::InputPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use core::time::Duration;
use lightyear::prelude::{client::*, server::*, *};
use lightyear_netcode::client_plugin::NetcodeConfig;
use lightyear_replication::receive::ReplicationReceiver;

pub const DET_SERVER_PORT: u16 = 56891;
pub const DET_SERVER_ADDR: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, DET_SERVER_PORT));
pub const DET_TICK_DURATION: Duration = Duration::from_millis(16);

/// Holder for a multi-client deterministic-replication test setup.
pub struct DetStepper {
    pub client_apps: Vec<App>,
    pub server_app: App,
    pub client_entities: Vec<Entity>,
    pub server_entity: Entity,
    pub client_of_entities: Vec<Entity>,
    pub protocol: DetProtocolPlugin,
    pub tick_duration: Duration,
    pub current_time: bevy::platform::time::Instant,
    pub frame_duration: Duration,
}

impl DetStepper {
    pub fn new_server() -> Self {
        Self::new_server_with_protocol(DetProtocolPlugin::default())
    }

    pub fn new_server_with_protocol(protocol: DetProtocolPlugin) -> Self {
        let tick_duration = DET_TICK_DURATION;
        let mut server_app = App::new();
        server_app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            StatesPlugin,
            InputPlugin,
            LogPlugin::default(),
            MetricsPlugin::new(None),
        ));
        server_app.add_plugins((server::ServerPlugins { tick_duration }, RoomPlugin));
        // `ChecksumPlugin` is registered by the deterministic protocol so
        // checksum setup stays in the shared protocol registration order.
        server_app.add_plugins(protocol);

        let server_entity = server_app
            .world_mut()
            .spawn(NetcodeServer::new(
                lightyear_netcode::server_plugin::NetcodeConfig::default(),
            ))
            .id();

        Self {
            client_apps: vec![],
            server_app,
            client_entities: vec![],
            server_entity,
            client_of_entities: vec![],
            protocol,
            tick_duration,
            current_time: bevy::platform::time::Instant::now(),
            frame_duration: tick_duration,
        }
    }

    /// Add an extra netcode client connected via crossbeam.
    pub fn new_client(&mut self) -> usize {
        let mut client_app = App::new();
        client_app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            StatesPlugin,
            InputPlugin,
            LogPlugin::default(),
            MetricsPlugin::new(None),
        ));
        client_app.add_plugins(client::ClientPlugins {
            tick_duration: self.tick_duration,
        });
        client_app.edit_schedule(PreUpdate, |schedule| {
            schedule.set_executor(SingleThreadedExecutor::new());
        });
        client_app.edit_schedule(Update, |schedule| {
            schedule.set_executor(SingleThreadedExecutor::new());
        });
        client_app.edit_schedule(PostUpdate, |schedule| {
            schedule.set_executor(SingleThreadedExecutor::new());
        });
        // `ChecksumPlugin` is registered by the deterministic protocol so
        // checksum setup stays in the shared protocol registration order.
        client_app.add_plugins(self.protocol);

        client_app.finish();
        client_app.cleanup();
        let client_id = self.client_entities.len();
        let (crossbeam_client, crossbeam_server) = lightyear_crossbeam::CrossbeamIo::new_pair();

        let auth = Authentication::Manual {
            server_addr: DET_SERVER_ADDR,
            protocol_id: Default::default(),
            private_key: Default::default(),
            client_id: client_id as u64,
        };

        let client_entity = client_app
            .world_mut()
            .spawn((
                Client,
                PingManager::new(PingConfig {
                    ping_interval: Duration::default(),
                }),
                ReplicationSender,
                ReplicationReceiver,
                crossbeam_client,
                PredictionManager {
                    rollback_policy: RollbackPolicy {
                        state: RollbackMode::Disabled,
                        input: RollbackMode::Check,
                        max_rollback_ticks: 100,
                    },
                    ..default()
                },
                {
                    let mut sync = SyncConfig::default();
                    // 2-tick margin (vs the default 1.0) is needed because
                    // the stepper runs every client app then the server app
                    // sequentially inside one "frame", giving the network
                    // no real flush window. See `stepper::test_input_timeline_config`.
                    sync.jitter_margin = 2.0;
                    InputTimelineConfig::default()
                        .with_input_delay(InputDelayConfig::fixed_input_delay(0))
                        .with_sync_config(sync)
                },
                NetcodeClient::new(auth, NetcodeConfig::default()).unwrap(),
            ))
            .id();

        let client_of_entity = self
            .server_app
            .world_mut()
            .spawn((
                LinkOf {
                    server: self.server_entity,
                },
                PingManager::new(PingConfig {
                    ping_interval: Duration::default(),
                }),
                ReplicationSender,
                ReplicationReceiver,
                Link::default(),
                PeerAddr(SocketAddr::new(
                    core::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
                    client_id as u16,
                )),
                Linked,
                crossbeam_server,
            ))
            .id();

        self.client_entities.push(client_entity);
        self.client_of_entities.push(client_of_entity);
        self.client_apps.push(client_app);
        client_id
    }

    pub fn client_app(&mut self, id: usize) -> &mut App {
        &mut self.client_apps[id]
    }

    pub fn client_tick(&self, id: usize) -> Tick {
        self.client_apps[id]
            .world()
            .resource::<LocalTimeline>()
            .tick()
    }

    pub fn server_tick(&self) -> Tick {
        self.server_app.world().resource::<LocalTimeline>().tick()
    }

    pub fn client(&self, id: usize) -> EntityRef<'_> {
        self.client_apps[id]
            .world()
            .entity(self.client_entities[id])
    }

    pub fn client_mut(&mut self, id: usize) -> EntityWorldMut<'_> {
        self.client_apps[id]
            .world_mut()
            .entity_mut(self.client_entities[id])
    }

    pub fn server(&self) -> EntityRef<'_> {
        self.server_app.world().entity(self.server_entity)
    }

    pub fn start(&mut self) {
        if matches!(
            self.server_app.plugins_state(),
            PluginsState::Ready | PluginsState::Adding
        ) {
            self.server_app.finish();
            self.server_app.cleanup();
        }

        let now = bevy::platform::time::Instant::now();
        self.current_time = now;
        self.server_app
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);
        self.server_app.world_mut().trigger(Start {
            entity: self.server_entity,
        });
        self.server_app.world_mut().flush();
    }

    pub fn connect_all(&mut self) {
        let now = self.current_time;
        for i in 0..self.client_entities.len() {
            self.client_apps[i]
                .world_mut()
                .get_resource_mut::<Time<Real>>()
                .unwrap()
                .update_with_instant(now);
            self.client_apps[i].world_mut().trigger(Connect {
                entity: self.client_entities[i],
            });
        }
        self.wait_for_connection();
        self.wait_for_sync();
    }

    /// Connect just a single client (used for late-join scenarios).
    /// Waits until that specific client is connected + synced.
    pub fn connect_single(&mut self, id: usize) {
        let now = self.current_time;
        self.client_apps[id]
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);
        self.client_apps[id].world_mut().trigger(Connect {
            entity: self.client_entities[id],
        });
        for _ in 0..200 {
            if self.client(id).contains::<Connected>()
                && self.client(id).contains::<IsSynced<InputTimeline>>()
            {
                info!("Client {} connected + synced", id);
                return;
            }
            self.frame_step(1);
        }
        panic!("Client {} failed to connect+sync in time", id);
    }

    /// Disconnect and remove the most recently added client.
    pub fn disconnect_last_client(&mut self) {
        let last = self.client_entities.len() - 1;
        let client_entity = self.client_entities[last];
        self.client_apps[last].world_mut().trigger(Disconnect {
            entity: client_entity,
        });

        // Let the in-memory disconnect packet reach the server so netcode
        // releases the peer id before a reconnect with the same id.
        self.frame_step(10);

        let client_entity = self.client_entities.pop().unwrap();
        let server_entity = self.client_of_entities.pop().unwrap();
        let mut client_app = self.client_apps.pop().unwrap();
        client_app.world_mut().flush();
        client_app.world_mut().despawn(client_entity);
        if self.server_app.world().get_entity(server_entity).is_ok() {
            self.server_app.world_mut().despawn(server_entity);
        }
        self.frame_step(1);
    }

    pub fn wait_for_connection(&mut self) {
        for _ in 0..100 {
            if (0..self.client_entities.len()).all(|id| self.client(id).contains::<Connected>()) {
                info!("All clients connected");
                return;
            }
            self.frame_step(1);
        }
        panic!("Clients failed to connect");
    }

    pub fn wait_for_sync(&mut self) {
        for _ in 0..100 {
            if (0..self.client_entities.len())
                .all(|id| self.client(id).contains::<IsSynced<InputTimeline>>())
            {
                info!("All clients synced");
                return;
            }
            self.frame_step(1);
        }
        panic!("Clients failed to sync");
    }

    pub fn advance_time(&mut self, duration: Duration) {
        self.current_time += duration;
        for client_app in &mut self.client_apps {
            client_app.insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        }
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
    }

    pub fn frame_step(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_time(self.frame_duration);
            for (i, client_app) in self.client_apps.iter_mut().enumerate() {
                error_span!("client", ?i).in_scope(|| client_app.update());
            }
            error_span!("server").in_scope(|| self.server_app.update());
        }
    }
}

/// Spawn the player entity + BEI action entity for the given client on
/// the server.
///
/// When `gated` is `true`, the player's physics components are hidden until
/// the client requests a bundled catch-up snapshot (matching
/// `examples/deterministic_replication`'s `handle_connected`). Use `false`
/// for `CatchUpMode::InputOnly` tests where clients rely on `replicate_once`
/// at spawn time to receive the initial state.
///
/// The action entity is spawned with `PreSpawned::new(hash)` so that the
/// client can spawn a local matching action entity with the same hash.
pub fn spawn_player_on_server(
    server_app: &mut App,
    peer_id: PeerId,
    spawn_xy: Vec2,
    gated: bool,
) -> Entity {
    use crate::client_server::deterministic::protocol::{
        DetMovement, DetPhysicsBundle, DetPlayerActivationTick, DetPlayerId, Player,
        action_prespawn_hash,
    };
    use bevy_enhanced_input::prelude::{Action, ActionOf};
    use lightyear_prediction::rollback::CatchUpGated;
    use lightyear_prediction::rollback::DeterministicPredicted;
    use lightyear_replication::prelude::PreSpawned;

    let mut entity = server_app.world_mut().spawn((
        Replicate::to_clients(NetworkTarget::All),
        DetPlayerId(peer_id),
        DetPlayerActivationTick::pending(),
        Player,
        Position::from_xy(spawn_xy.x, spawn_xy.y),
        Rotation::default(),
        LinearVelocity::default(),
        AngularVelocity::default(),
        DetPhysicsBundle::player(),
        DeterministicPredicted {
            skip_despawn: true,
            ..default()
        },
        Name::from("Player"),
    ));
    if gated {
        entity.insert(CatchUpGated);
    }
    let player_entity = entity.id();

    // Spawn the action entity on the server. Replicate it to every client
    // EXCEPT the owning peer — the owning client spawns a matching PreSpawned
    // action entity locally via `spawn_local_action_on_client`.
    //
    // Why AllExcept instead of All: replicon's PreSpawned signature matching
    // only fires when the server's entity-mapping message arrives AT OR AFTER
    // the client has registered its local prespawn in `SignatureMap`. In this
    // test, the stepper lets replication settle (`frame_step(15)`) before
    // calling `spawn_local_action_on_client`, so if the server action were
    // replicated to the owning client it would have already been materialized
    // as a new (non-prespawn) entity and the later-arriving local prespawn
    // would never merge with it. The owning client would then have TWO action
    // entities (replicated + prespawn) for the same (Player, Action) pair,
    // causing `apply_movement` to fire twice per tick and desynchronize the
    // simulation (most visibly as a checksum divergence the moment remote
    // inputs start arriving). Sending the server action only to the non-
    // owning clients keeps each client with exactly one action entity per
    // player. See `examples/bevy_enhanced_inputs` for the alternative pattern
    // where the prespawn is spawned immediately on `Connected` to stay ahead
    // of replication.
    let hash = action_prespawn_hash(peer_id);
    server_app.world_mut().spawn((
        ActionOf::<Player>::new(player_entity),
        Action::<DetMovement>::new(),
        PreSpawned::new(hash),
        Replicate::to_clients(NetworkTarget::AllExceptSingle(peer_id)),
    ));

    player_entity
}

/// Spawn the local action entity on the given client, matching the server
/// via `PreSpawned` hash, and insert `InputMarker::<Player>` on the
/// player (context) entity so BEI knows this is the locally-owned
/// context. Called once the client has received its own
/// `DetPlayerId` entity.
pub fn spawn_local_action_on_client(
    client_app: &mut App,
    client_player_entity: Entity,
    peer_id: PeerId,
) -> Entity {
    use crate::client_server::deterministic::protocol::{
        DetBuffer, DetMovement, Player, action_prespawn_hash,
    };
    use bevy_enhanced_input::prelude::{Action, ActionOf};
    use lightyear::prelude::input::bei::InputMarker;
    use lightyear_replication::prelude::PreSpawned;

    // InputMarker goes on the context entity (player), not the action entity.
    client_app
        .world_mut()
        .entity_mut(client_player_entity)
        .insert(InputMarker::<Player>::default());

    let hash = action_prespawn_hash(peer_id);
    // Seed with a disabled `ActionMock` so tests can later write into it
    // without dealing with inserting the component. `enabled=false` means
    // BEI ignores it until a system flips it on.
    use bevy_enhanced_input::prelude::ActionMock;
    client_app
        .world_mut()
        .spawn((
            ActionOf::<Player>::new(client_player_entity),
            Action::<DetMovement>::new(),
            DetBuffer::default(),
            PreSpawned::new(hash).for_receiver(client_player_entity),
            ActionMock::default(),
            InputMarker::<Player>::default(),
        ))
        .id()
}
