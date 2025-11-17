use core::f32::consts::TAU;
use std::env;
use std::thread;

use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;
use leafwing_input_manager::action_diff::ActionDiff;
use leafwing_input_manager::prelude::*;
use lightyear::connection::client::PeerMetadata;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::{FIXED_TIMESTEP_HZ, SEND_INTERVAL};

use crate::protocol::*;
use crate::shared;
use crate::shared::{apply_action_state_to_player_movement, color_from_id};

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_observer(handle_new_client);
        app.add_observer(handle_connections);
        app.add_systems(
            Update,
            (update_player_metrics.run_if(on_timer(Duration::from_secs(1))),),
        );

        app.add_systems(
            FixedUpdate,
            handle_hit_event
                .run_if(on_message::<BulletHitMessage>)
                .after(shared::process_collisions),
        );

        let stall_config = ServerStallStress::from_env();
        if stall_config.enabled() {
            app.insert_resource(stall_config);
            app.add_systems(FixedUpdate, server_stall_system.before(handle_hit_event));
        }
    }
}

/// Since Player is replicated, this allows the clients to display remote players' latency stats.
fn update_player_metrics(
    links: Query<&Link, With<LinkOf>>,
    mut q: Query<(&mut Player, &ControlledBy)>,
) {
    for (mut player, controlled) in q.iter_mut() {
        if let Ok(link) = links.get(controlled.owner) {
            player.rtt = link.stats.rtt;
            player.jitter = link.stats.jitter;
        }
    }
}

fn init(mut commands: Commands) {
    // the balls are server-authoritative
    const NUM_BALLS: usize = 6;
    for i in 0..NUM_BALLS {
        let radius = 10.0 + i as f32 * 4.0;
        let angle: f32 = i as f32 * (TAU / NUM_BALLS as f32);
        let pos = Vec2::new(125.0 * angle.cos(), 125.0 * angle.sin());
        let ball = BallMarker::new(radius);
        commands.spawn((
            Position(pos),
            ColorComponent(css::GOLD.into()),
            ball.physics_bundle(),
            ball,
            Name::new("Ball"),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ));
    }
}

/// Add the ReplicationSender component to new clients
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert(ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));
}

/// Whenever a new client connects, spawn their spaceship
pub(crate) fn handle_connections(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
    all_players: Query<Entity, With<Player>>,
) {
    // track the number of connected players in order to pick colors and starting positions
    let player_n = all_players.iter().count();
    if let Ok(remote_id) = query.get(trigger.entity) {
        let client_id = remote_id.0;
        info!("New connected client, client_id: {client_id:?}. Spawning player entity..");
        // pick color and x,y pos for player
        let available_colors = [
            css::LIMEGREEN,
            css::PINK,
            css::YELLOW,
            css::AQUA,
            css::CRIMSON,
            css::GOLD,
            css::ORANGE_RED,
            css::SILVER,
            css::SALMON,
            css::YELLOW_GREEN,
            css::WHITE,
            css::RED,
        ];
        let col = available_colors[player_n % available_colors.len()];
        let angle: f32 = player_n as f32 * 5.0;
        let x = 200.0 * angle.cos();
        let y = 200.0 * angle.sin();

        // spawn the player with ActionState - the client will add their own InputMap
        let player_ent = commands
            .spawn((
                Player::new(client_id, pick_player_name(client_id.to_bits())),
                Score(0),
                Name::new("Player"),
                ActionState::<PlayerActions>::default(),
                Position(Vec2::new(x, y)),
                Replicate::to_clients(NetworkTarget::All),
                PredictionTarget::to_clients(NetworkTarget::All),
                ControlledBy {
                    owner: trigger.entity,
                    lifetime: Default::default(),
                },
                // prevent rendering children to be replicated
                DisableReplicateHierarchy,
                PhysicsBundle::player_ship(),
                Weapon::new((FIXED_TIMESTEP_HZ / 5.0) as u16),
                ColorComponent(col.into()),
            ))
            .id();
        info!("Created entity {player_ent:?} for client {client_id:?}");
    }
}

fn pick_player_name(client_id: u64) -> String {
    let index = (client_id % NAMES.len() as u64) as usize;
    NAMES[index].to_string()
}

const NAMES: [&str; 35] = [
    "Ellen Ripley",
    "Sarah Connor",
    "Neo",
    "Trinity",
    "Morpheus",
    "John Connor",
    "T-1000",
    "Rick Deckard",
    "Princess Leia",
    "Han Solo",
    "Spock",
    "James T. Kirk",
    "Hikaru Sulu",
    "Nyota Uhura",
    "Jean-Luc Picard",
    "Data",
    "Beverly Crusher",
    "Seven of Nine",
    "Doctor Who",
    "Rose Tyler",
    "Marty McFly",
    "Doc Brown",
    "Dana Scully",
    "Fox Mulder",
    "Riddick",
    "Barbarella",
    "HAL 9000",
    "Megatron",
    "Furiosa",
    "Lois Lane",
    "Clark Kent",
    "Tony Stark",
    "Natasha Romanoff",
    "Bruce Banner",
    "Mr. T",
];

#[derive(Resource)]
struct ServerStallStress {
    duration: Duration,
    interval_ticks: u32,
    last_stall_tick: Option<Tick>,
}

impl Default for ServerStallStress {
    fn default() -> Self {
        Self {
            duration: Duration::ZERO,
            interval_ticks: 0,
            last_stall_tick: None,
        }
    }
}

impl ServerStallStress {
    fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Some(ms) = env::var("SPACESHIPS_SERVER_STALL_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            cfg.duration = Duration::from_millis(ms);
        }
        if let Some(interval) = env::var("SPACESHIPS_SERVER_STALL_INTERVAL_TICKS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        {
            cfg.interval_ticks = interval.max(1);
        }
        cfg
    }

    fn enabled(&self) -> bool {
        self.duration > Duration::ZERO && self.interval_ticks > 0
    }
}

fn server_stall_system(
    mut stall: ResMut<ServerStallStress>,
    local_timeline: Single<&LocalTimeline, With<Server>>,
) {
    if !stall.enabled() {
        return;
    }
    let tick = local_timeline.tick();
    if let Some(last) = stall.last_stall_tick {
        let delta = tick.0.wrapping_sub(last.0);
        if delta < stall.interval_ticks as u16 {
            return;
        }
    }
    info!(
        "Server stall stress: sleeping {:?} at tick {}",
        stall.duration, tick.0
    );
    thread::sleep(stall.duration);
    stall.last_stall_tick = Some(tick);
}

/// Server will manipulate scores when a bullet collides with a player.
/// the `Score` component is a simple replication. Score is fully server-authoritative.
pub(crate) fn handle_hit_event(
    peer_metadata: Res<PeerMetadata>,
    mut events: MessageReader<BulletHitMessage>,
    mut player_q: Query<(&Player, &mut Score)>,
) {
    let client_id_to_player_entity =
        |client_id: PeerId| -> Option<Entity> { peer_metadata.mapping.get(&client_id).copied() };

    for ev in events.read() {
        // did they hit a player?
        if let Some(victim_entity) = ev.victim_client_id.and_then(client_id_to_player_entity) {
            if let Ok((player, mut score)) = player_q.get_mut(victim_entity) {
                score.0 -= 1;
            }
            if let Some(shooter_entity) = client_id_to_player_entity(ev.bullet_owner)
                && let Ok((player, mut score)) = player_q.get_mut(shooter_entity)
            {
                score.0 += 1;
            }
        }
    }
}
