use core::f32::consts::TAU;

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
use crate::shared::ApplyInputsQuery;
use crate::shared::ApplyInputsQueryItem;
use crate::shared::{apply_action_state_to_player_movement, color_from_id};

// Plugin for server-specific logic
pub struct ExampleServerPlugin {
    pub(crate) predict_all: bool,
}

#[derive(Resource)]
pub struct Global {
    predict_all: bool,
}

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Global {
            predict_all: self.predict_all,
        });
        app.add_systems(Startup, init);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(
            FixedUpdate,
            (player_movement, shared::shared_player_firing).chain(),
        );
        app.add_observer(handle_new_client);
        app.add_observer(handle_connections);
        app.add_systems(
            Update,
            (update_player_metrics.run_if(on_timer(Duration::from_secs(1))),),
        );

        app.add_systems(
            FixedUpdate,
            handle_hit_event
                .run_if(on_event::<BulletHitEvent>)
                .after(shared::process_collisions),
        );
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
pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.target())
        .insert(ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));
}

/// Whenever a new client connects, spawn their spaceship
pub(crate) fn handle_connections(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
    all_players: Query<Entity, With<Player>>,
) {
    // track the number of connected players in order to pick colors and starting positions
    let player_n = all_players.iter().count();
    if let Ok(remote_id) = query.get(trigger.target()) {
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
                    owner: trigger.target(),
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

/// Server will manipulate scores when a bullet collides with a player.
/// the `Score` component is a simple replication. Score is fully server-authoritative.
pub(crate) fn handle_hit_event(
    peer_metadata: Res<PeerMetadata>,
    mut events: EventReader<BulletHitEvent>,
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
            if let Some(shooter_entity) = client_id_to_player_entity(ev.bullet_owner) {
                if let Ok((player, mut score)) = player_q.get_mut(shooter_entity) {
                    score.0 += 1;
                }
            }
        }
    }
}

/// Read inputs and move players
///
/// If we didn't receive the input for a given player, we do nothing (which is the default behaviour from lightyear),
/// which means that we will be using the last known input for that player
/// (i.e. we consider that the player kept pressing the same keys).
/// see: https://github.com/cBournhonesque/lightyear/issues/492
pub(crate) fn player_movement(
    mut q: Query<(&ActionState<PlayerActions>, ApplyInputsQuery), With<Player>>,
    timeline: Single<&LocalTimeline, With<Server>>,
) {
    let tick = timeline.tick();
    for (action_state, mut aiq) in q.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            trace!(
                "🎹 {:?} {tick:?} = {:?}",
                aiq.player.client_id,
                action_state.get_pressed(),
            );
        }
        apply_action_state_to_player_movement(action_state, &mut aiq, tick);
    }
}
