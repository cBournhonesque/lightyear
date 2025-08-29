use crate::client::ExampleClientPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::{BOT_RADIUS, Rooms, SharedPlugin, color_from_id};
use avian2d::prelude::*;
use bevy::input::InputPlugin;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use bevy_enhanced_input::EnhancedInputSet;
use bevy_enhanced_input::prelude::{Actions, Completed, Started};
use core::ops::DerefMut;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::crossbeam::CrossbeamIo;
use lightyear::interpolation::plugin::InterpolationDelay;
use lightyear::netcode::NetcodeClient;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::{
    LagCompensationHistory, LagCompensationPlugin, LagCompensationSet, LagCompensationSpatialQuery,
};
use lightyear_examples_common::cli::new_headless_app;
use lightyear_examples_common::shared::{SEND_INTERVAL, SERVER_ADDR, SHARED_SETTINGS};
use rand::random;
use std::net::SocketAddr;

pub struct ExampleServerPlugin;

const BULLET_COLLISION_DISTANCE_CHECK: f32 = 4.0;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RoomPlugin);
        app.init_resource::<Rooms>();

        app.add_plugins(LagCompensationPlugin);
        app.add_observer(handle_new_client);
        app.add_observer(spawn_player);
        app.add_observer(cycle_replication_mode);
        app.add_observer(cycle_projectile_mode);

        app.add_systems(Startup, spawn_global_control);

        // we don't want to panic when trying to read the InputReader if gui is not enabled
        app.configure_sets(PreUpdate, EnhancedInputSet::Prepare.run_if(|| false));
        app.add_plugins(BotPlugin);
    }
}

/// Spawn bots when the server starts
/// NOTE: this has to be done after `Plugin::finish()` so that BEI has finished building.
pub(crate) fn spawn_bots(mut commands: Commands) {
    commands.trigger(SpawnBot);
}

pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    info!(
        "Adding ReplicationSender to new ClientOf entity: {:?}",
        trigger.target()
    );
    commands.entity(trigger.target()).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        // We need a ReplicationReceiver on the server side because the Action entities are spawned
        // on the client and replicated to the server.
        ReplicationReceiver::default(),
        Name::from("ClientOf"),
    ));
}

pub(crate) fn spawn_global_control(mut commands: Commands) {
    commands.spawn((
        ClientContext,
        Replicate::to_clients(NetworkTarget::All),
        GameReplicationMode::default(),
        ProjectileReplicationMode::default(),
        Name::new("ClientContext"),
    ));
}

// Replicate the pre-spawned entities back to the client
// We have to use `InitialReplicated` instead of `Replicated`, because
// the server has already assumed authority over the entity so the `Replicated` component
// has been removed
pub(crate) fn spawn_player(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<(&RemoteId, Has<BotClient>), With<ClientOf>>,
    mut commands: Commands,
    mut rooms: ResMut<Rooms>,
    replicated_players: Query<
        (Entity, &InitialReplicated),
        (Added<InitialReplicated>, With<PlayerId>),
    >,
) {
    let sender = trigger.target();
    let Ok((client_id, is_bot)) = query.get(sender) else {
        return;
    };
    let client_id = client_id.0;
    info!("Spawning player with id: {}", client_id);

    for i in 0..1 {
        let replication_mode = GameReplicationMode::from_room_id(i);
        let room = *rooms.rooms.entry(replication_mode).or_insert_with(|| {
            commands
                .spawn((
                    Room::default(),
                    Name::new(format!("Room{}", replication_mode.name())),
                ))
                .id()
        });

        // start by adding the player to the first room
        if i == 0 {
            commands
                .entity(room)
                .trigger(RoomEvent::AddSender(trigger.target()));
        }
        let player = server_player_bundle(room, client_id, sender, replication_mode);
        let player_entity = match replication_mode {
            GameReplicationMode::AllPredicted => {
                commands.spawn((player, PredictionTarget::to_clients(NetworkTarget::All)))
            }
            GameReplicationMode::ClientPredictedNoComp => commands.spawn((
                player,
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            )),
            GameReplicationMode::ClientPredictedLagComp => commands.spawn((
                player,
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            )),
            GameReplicationMode::ClientSideHitDetection => commands.spawn((
                player,
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            )),
            GameReplicationMode::AllInterpolated => {
                commands.spawn((player, InterpolationTarget::to_clients(NetworkTarget::All)))
            }
            GameReplicationMode::OnlyInputsReplicated => commands.spawn((
                PlayerContext,
                Replicate::to_clients(NetworkTarget::All),
                NetworkVisibility::default(),
                ControlledBy {
                    owner: sender,
                    lifetime: Default::default(),
                },
                PlayerMarker,
                Name::new("Player"),
            )),
        }
        .id();
        if is_bot {
            commands.entity(player_entity).insert(Bot);
        }
        info!("Spawning player {player_entity:?} for room: {room:?}");
        commands
            .entity(room)
            .trigger(RoomEvent::AddEntity(player_entity));
    }
}

fn server_player_bundle(
    room: Entity,
    client_id: PeerId,
    owner: Entity,
    replication_mode: GameReplicationMode,
) -> impl Bundle {
    let bundle = shared::player_bundle(client_id);
    (
        Replicate::to_clients(NetworkTarget::All),
        NetworkVisibility::default(),
        ControlledBy {
            owner,
            lifetime: Default::default(),
        },
        replication_mode,
        bundle,
    )
}

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {}

    // run in `cleanup` because BEI finishes building in `finish`
    fn cleanup(&self, app: &mut App) {
        app.add_observer(spawn_bot_app);
        app.add_systems(Startup, spawn_bots);
    }
}

#[derive(Component)]
pub struct BotClient;

#[derive(Event)]
pub struct SpawnBot;

pub struct BotApp(App);

unsafe impl Send for BotApp {}
unsafe impl Sync for BotApp {}

impl BotApp {
    fn run(&mut self) {
        info_span!("bot").in_scope(|| {
            self.0.run();
        });
    }
}

/// On the server, we will create a second app to host a bot that is similar to a real client,
/// but their inputs are mocked
fn spawn_bot_app(
    trigger: Trigger<SpawnBot>,
    tick_duration: Res<TickDuration>,
    server: Single<Entity, With<Server>>,
    mut commands: Commands,
) {
    info!("Spawning bot app");
    let (crossbeam_client, crossbeam_server) = CrossbeamIo::new_pair();

    let mut app = new_headless_app();
    // TODO: just spawn a bot player entity without creating a new client
    // cannot use headless app because the frame rate is too fast so
    // the bot sends too many packets
    // let mut app = new_gui_app(false);
    app.add_plugins(InputPlugin);
    app.add_plugins(lightyear::prelude::client::ClientPlugins {
        tick_duration: tick_duration.0,
    });
    app.add_plugins(SharedPlugin);
    app.add_plugins(ExampleClientPlugin);

    let client_id = rand::random::<u64>();
    let auth = Authentication::Manual {
        server_addr: SERVER_ADDR,
        client_id,
        private_key: SHARED_SETTINGS.private_key,
        protocol_id: SHARED_SETTINGS.protocol_id,
    };

    app.world_mut().spawn((
        Client::default(),
        BotClient,
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        ReplicationReceiver::default(),
        NetcodeClient::new(
            auth,
            lightyear::netcode::client_plugin::NetcodeConfig::default(),
        )
        .unwrap(),
        crossbeam_client,
        PredictionManager::default(),
        InterpolationManager::default(),
        Name::from("BotClient"),
    ));
    let server = server.into_inner();
    let conditioner = RecvLinkConditioner::new(LinkConditionerConfig::average_condition());
    commands.spawn((
        LinkOf { server },
        Link::new(Some(conditioner)),
        Linked,
        ClientOf,
        BotClient,
        crossbeam_server,
        ReplicationSender::default(),
    ));

    app.add_systems(Startup, bot_connect);
    app.add_systems(First, bot_inputs);
    #[cfg(not(feature = "gui"))]
    app.add_systems(Update, bot_wait);
    let mut bot_app = BotApp(app);
    std::thread::spawn(move || {
        bot_app.run();
    });
}

fn bot_connect(bot: Single<Entity, (With<BotClient>, With<Client>)>, mut commands: Commands) {
    let entity = bot.into_inner();
    info!("Bot entity {entity:?} connecting to server");
    commands.entity(entity).trigger(Connect);
}

#[derive(Debug, Clone, Copy, Default)]
enum BotMovementMode {
    #[default]
    Strafing, // 200ms intervals
    StraightLine, // 1s intervals
}

impl BotMovementMode {
    fn interval(&self) -> f32 {
        match self {
            BotMovementMode::Strafing => 0.2,     // 200ms for strafing
            BotMovementMode::StraightLine => 1.0, // 1s for straight line
        }
    }

    fn name(&self) -> &'static str {
        match self {
            BotMovementMode::Strafing => "strafing",
            BotMovementMode::StraightLine => "straight-line",
        }
    }
}

#[derive(Default)]
struct BotLocal {
    mode_timer: Stopwatch,
    key_timer: Stopwatch,
    current_mode: BotMovementMode,
    press_a: bool,
    override_direction: Option<bool>, // None = normal, Some(true) = force A, Some(false) = force D
}

fn bot_inputs(
    time: Res<Time>,
    mut input: ResMut<ButtonInput<KeyCode>>,
    player: Single<&Position, (With<Controlled>, Without<Confirmed>)>,
    mut local: Local<BotLocal>,
) {
    let BotLocal {
        mode_timer,
        key_timer,
        current_mode,
        press_a,
        override_direction,
    } = local.deref_mut();

    // If bot is too far from x = 0, override direction
    let threshold = 500.0;
    let pos_x = player.x;
    if pos_x.abs() > threshold {
        // If too far right, press A; if too far left, press D
        *override_direction = Some(pos_x > 0.0);
    } else if override_direction.is_some() && pos_x.abs() <= threshold * 0.8 {
        // If bot is close enough to center, resume normal strafing
        *override_direction = None;
    }

    mode_timer.tick(time.delta());
    key_timer.tick(time.delta());

    // Switch modes every 4 seconds
    if mode_timer.elapsed_secs() >= 4.0 {
        mode_timer.reset();
        *current_mode = match *current_mode {
            BotMovementMode::Strafing => BotMovementMode::StraightLine,
            BotMovementMode::StraightLine => BotMovementMode::Strafing,
        };
        trace!("Bot switching to {} mode", current_mode.name());
    }

    // Switch keys based on the current mode's interval
    if key_timer.elapsed_secs() >= current_mode.interval() {
        key_timer.reset();
        if *press_a {
            input.release(KeyCode::KeyA);
        } else {
            input.release(KeyCode::KeyD);
        }
        *press_a = !*press_a;
    }

    // Decide which key to press
    let press_a_now = match *override_direction {
        Some(true) => true,   // Press A to move left
        Some(false) => false, // Press D to move right
        None => *press_a,
    };

    // Press the current key
    if press_a_now {
        input.press(KeyCode::KeyA);
        input.release(KeyCode::KeyD);
    } else {
        input.press(KeyCode::KeyD);
        input.release(KeyCode::KeyA);
    }
    trace!(
        "Bot in {} mode, pressing {:?}",
        current_mode.name(),
        input.get_pressed().collect::<Vec<_>>()
    );
}

#[cfg(not(feature = "gui"))]
// prevent the bot from running too fast
fn bot_wait(timeline: Single<&LocalTimeline>) {
    std::thread::sleep(Duration::from_millis(15));
}

/// Handle room switching when replication mode changes
pub fn cycle_replication_mode(
    trigger: Trigger<Completed<CycleReplicationMode>>,
    global: Single<&mut GameReplicationMode, With<ClientContext>>,
    rooms: Res<Rooms>,
    clients: Query<Entity, With<ClientOf>>,
    mut commands: Commands,
) {
    let mut replication_mode = global.into_inner();
    let current_mode = *replication_mode;
    *replication_mode = replication_mode.next();

    // Move all clients from current room to next room
    if let (Some(current_room), Some(next_room)) = (
        rooms.rooms.get(&current_mode),
        rooms.rooms.get(&*replication_mode),
    ) {
        for client_entity in clients.iter() {
            commands.trigger_targets(RoomEvent::RemoveSender(client_entity), *current_room);
            commands.trigger_targets(RoomEvent::AddSender(client_entity), *next_room);
            info!(
                "Switching client {client_entity:?} from room {current_room:?} to room {next_room:?}"
            );
        }
    }

    info!("Cycled to replication mode: {}", replication_mode.name());
}

/// Handle cycling through projectile replication modes
pub fn cycle_projectile_mode(
    trigger: Trigger<Completed<CycleProjectileMode>>,
    global: Single<&mut ProjectileReplicationMode, With<ClientContext>>,
) {
    let mut projectile_mode = global.into_inner();
    *projectile_mode = projectile_mode.next();
    info!("Cycled to projectile mode: {}", projectile_mode.name());
}
