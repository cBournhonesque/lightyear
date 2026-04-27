use crate::automation::AutomationServerPlugin;
#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::{GameRooms, SharedPlugin, color_from_id};
use avian2d::prelude::*;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use bevy_enhanced_input::EnhancedInputSystems;
use bevy_enhanced_input::prelude::*;
use core::net::SocketAddr;
use core::ops::DerefMut;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::connection::client::PeerMetadata;
use lightyear::core::tick::TickDuration;
use lightyear::crossbeam::CrossbeamIo;
use lightyear::input::config::InputConfig;
use lightyear::input::server::{InputSystems as ServerInputSystems, ServerInputConfig};
use lightyear::interpolation::plugin::InterpolationDelay;
use lightyear::netcode::NetcodeClient;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::{
    LagCompensationHistory, LagCompensationPlugin, LagCompensationSpatialQuery,
};
use lightyear_examples_common::cli::new_headless_app;
use lightyear_examples_common::shared::{SEND_INTERVAL, SERVER_ADDR, SHARED_SETTINGS};
use rand::random;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationServerPlugin);
        app.add_plugins(RoomPlugin);
        app.init_resource::<GlobalActionLatch>();
        app.init_resource::<GameRooms>();
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));

        app.add_plugins(LagCompensationPlugin);
        app.add_observer(handle_new_client);
        app.add_observer(spawn_player);
        app.add_observer(release_global_action::<CycleReplicationMode>);
        app.add_observer(release_global_action::<CycleProjectileMode>);
        app.add_observer(release_global_action::<CycleWeapon>);
        app.add_observer(handle_hits);

        app.add_systems(Startup, spawn_global_control);
        app.add_systems(
            FixedPreUpdate,
            apply_global_action_inputs.after(ServerInputSystems::UpdateActionState),
        );

        // we don't want to panic when trying to read the InputReader if gui is not enabled
        app.configure_sets(PreUpdate, EnhancedInputSystems::Prepare.run_if(|| false));
        #[cfg(feature = "client")]
        app.add_plugins(bot::BotPlugin);
    }
}

pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    info!(
        "Adding ReplicationSender to new ClientOf entity: {:?}",
        trigger.entity
    );
    commands.entity(trigger.entity).insert((
        ReplicationSender,
        // We need a ReplicationReceiver on the server side because the Action entities are spawned
        // on the client and replicated to the server.
        ReplicationReceiver,
        Name::from("ClientOf"),
    ));
}

pub(crate) fn spawn_global_control(mut commands: Commands) {
    let global = commands
        .spawn((
            ClientContext,
            Replicate::to_clients(NetworkTarget::All),
            GameReplicationMode::default(),
            ProjectileReplicationMode::default(),
            WeaponType::default(),
            Name::new("ClientContext"),
        ))
        .id();
    shared::spawn_global_actions(&mut commands, global, true);
}

#[derive(Resource, Default)]
struct GlobalActionLatch {
    active: HashSet<Entity>,
}

impl GlobalActionLatch {
    fn start(&mut self, action: Entity) -> bool {
        self.active.insert(action)
    }

    fn complete(&mut self, action: Entity) {
        self.active.remove(&action);
    }
}

fn release_global_action<A: InputAction>(
    trigger: On<Complete<A>>,
    mut latch: ResMut<GlobalActionLatch>,
) {
    latch.complete(trigger.action);
}

fn take_fired_once<A: InputAction>(
    actions: &Query<(Entity, &ActionEvents), With<Action<A>>>,
    latch: &mut GlobalActionLatch,
) -> bool {
    let mut fired = false;
    for (entity, events) in actions {
        if events.contains(ActionEvents::COMPLETE) || events.contains(ActionEvents::CANCEL) {
            latch.complete(entity);
        }
        if events.contains(ActionEvents::FIRE) && latch.start(entity) {
            fired = true;
        }
    }
    fired
}

fn apply_global_action_inputs(
    mut global: Query<
        (
            &mut GameReplicationMode,
            &mut ProjectileReplicationMode,
            &mut WeaponType,
        ),
        With<ClientContext>,
    >,
    rooms: Res<GameRooms>,
    mut input_config: ResMut<ServerInputConfig<PlayerContext>>,
    clients: Query<Entity, With<ClientOf>>,
    players: Query<Entity, With<PlayerMarker>>,
    replication_actions: Query<(Entity, &ActionEvents), With<Action<CycleReplicationMode>>>,
    projectile_actions: Query<(Entity, &ActionEvents), With<Action<CycleProjectileMode>>>,
    weapon_actions: Query<(Entity, &ActionEvents), With<Action<CycleWeapon>>>,
    mut latch: ResMut<GlobalActionLatch>,
    mut commands: Commands,
) {
    let Ok((mut replication_mode, mut projectile_mode, mut weapon_type)) = global.single_mut()
    else {
        return;
    };

    if take_fired_once(&replication_actions, &mut latch) {
        apply_replication_mode_cycle(
            &mut replication_mode,
            &rooms,
            &mut input_config,
            &clients,
            &players,
            &mut commands,
        );
    }
    if take_fired_once(&projectile_actions, &mut latch) {
        *projectile_mode = projectile_mode.next();
        info!("Cycled to projectile mode: {}", projectile_mode.name());
    }
    if take_fired_once(&weapon_actions, &mut latch) {
        *weapon_type = weapon_type.next();
        info!("Switched to weapon: {}", weapon_type.name());
    }
}

pub(crate) fn spawn_player(
    trigger: On<Add, Connected>,
    query: Query<(&RemoteId, Has<bot::BotClient>), With<ClientOf>>,
    mut commands: Commands,
    mut rooms: ResMut<GameRooms>,
    mut room_allocator: ResMut<RoomAllocator>,
) {
    let sender = trigger.entity;
    let Ok((client_id, is_bot)) = query.get(sender) else {
        return;
    };
    let client_id = client_id.0;
    info!("Spawning player with id: {}", client_id);

    for i in 0..6 {
        let replication_mode = GameReplicationMode::from_room_id(i);
        let room_id = *rooms
            .rooms
            .entry(replication_mode)
            .or_insert_with(|| room_allocator.allocate());

        // start by adding the player to the first room
        if i == 0 {
            commands
                .entity(trigger.entity)
                .insert(lightyear::prelude::Rooms::single(room_id));
        }
        let player = server_player_bundle(room_id, client_id, sender, replication_mode);
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
                // add the component to make lag-compensation possible!
                LagCompensationHistory::default(),
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
                replication_mode,
                Replicate::to_clients(NetworkTarget::All),
                lightyear::prelude::Rooms::single(room_id),
                ControlledBy {
                    owner: sender,
                    lifetime: Default::default(),
                },
                PlayerMarker,
                PlayerId(client_id),
                Name::new("Player"),
            )),
        }
        .id();
        if is_bot {
            commands.entity(player_entity).insert(Bot);
        }
        shared::spawn_player_actions(
            &mut commands,
            player_entity,
            client_id,
            replication_mode,
            true,
        );
        info!("Spawning player {player_entity:?} for room: {room_id:?}");
    }
}

fn server_player_bundle(
    room_id: RoomId,
    client_id: PeerId,
    owner: Entity,
    replication_mode: GameReplicationMode,
) -> impl Bundle {
    let bundle = shared::player_bundle(client_id, replication_mode);
    (
        Replicate::to_clients(NetworkTarget::All),
        lightyear::prelude::Rooms::single(room_id),
        ControlledBy {
            owner,
            lifetime: Default::default(),
        },
        bundle,
    )
}

/// Increment the score if the client told us about a detected hit.
fn handle_hits(trigger: On<RemoteEvent<HitDetected>>, mut scores: Query<&mut Score>) {
    // TODO: ideally we would also despawn the bullet, otherwise we will keep replicating data for it to clients
    //  even though they have already despawned it!
    if let Ok(mut score) = scores.get_mut(trigger.trigger.shooter) {
        info!(
            ?trigger,
            "Server received hit detection trigger from client!"
        );
        score.0 += 1;
    }
}

#[cfg(feature = "client")]
mod bot {
    use super::*;
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

    /// Spawn bots when the server starts
    /// NOTE: this has to be done after `Plugin::finish()` so that BEI has finished building.
    pub(crate) fn spawn_bots(mut commands: Commands) {
        commands.trigger(SpawnBot);
    }

    /// On the server, we will create a second app to host a bot that is similar to a real client,
    /// but their inputs are mocked
    fn spawn_bot_app(
        trigger: On<SpawnBot>,
        tick_duration: Res<TickDuration>,
        server: Single<Entity, With<Server>>,
        mut commands: Commands,
    ) {
        info!("Spawning bot app");
        let (crossbeam_client, crossbeam_server) = CrossbeamIo::new_pair();

        // Bots are client apps; pace their main loop to ~60 FPS so they
        // don't flood the server with packets under the
        // MinimalPlugins-default run-as-fast-as-possible scheduler.
        let mut app = new_headless_app(Some(Duration::from_secs_f64(
            1.0 / lightyear_examples_common::cli::HEADLESS_CLIENT_LOOP_HZ,
        )));
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
            ReplicationSender,
            ReplicationReceiver,
            NetcodeClient::new(
                auth,
                lightyear::netcode::client_plugin::NetcodeConfig::default(),
            )
            .unwrap(),
            crossbeam_client,
            PredictionManager::default(),
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
            ReplicationSender,
        ));

        app.add_systems(Startup, bot_connect);
        app.add_systems(FixedFirst, bot_inputs.run_if(not(is_in_rollback)));
        app.add_systems(Update, bot_wait);
        let mut bot_app = BotApp(app);
        std::thread::spawn(move || {
            bot_app.run();
        });
    }

    fn bot_connect(bot: Single<Entity, (With<BotClient>, With<Client>)>, mut commands: Commands) {
        let entity = bot.into_inner();
        info!("Bot entity {entity:?} connecting to server");
        commands.trigger(Connect { entity });
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
                BotMovementMode::Strafing => 0.4,     // 400ms for strafing
                BotMovementMode::StraightLine => 2.0, // 2s for straight line
            }
        }

        fn name(&self) -> &'static str {
            match self {
                BotMovementMode::Strafing => "strafing",
                BotMovementMode::StraightLine => "straight-line",
            }
        }
    }

    struct BotLocal {
        mode_timer: Stopwatch,
        key_timer: Stopwatch,
        shoot_timer: Timer,
        current_mode: BotMovementMode,
        press_a: bool,
        override_direction: Option<bool>, // None = normal, Some(true) = force A, Some(false) = force D
    }

    impl Default for BotLocal {
        fn default() -> Self {
            BotLocal {
                mode_timer: Default::default(),
                key_timer: Default::default(),
                shoot_timer: Timer::from_seconds(2.0, TimerMode::Repeating),
                current_mode: Default::default(),
                press_a: false,
                override_direction: None,
            }
        }
    }

    fn bot_inputs(
        time: Res<Time>,
        mut input: ResMut<ButtonInput<KeyCode>>,
        player: Single<&Position, (With<Controlled>, With<PlayerMarker>)>,
        mut local: Local<BotLocal>,
    ) {
        let BotLocal {
            mode_timer,
            key_timer,
            shoot_timer,
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
        shoot_timer.tick(time.delta());

        // we use press-space to make sure that we press the button long enough for it to be captured in FixedUpdate
        if shoot_timer.is_finished() {
            input.press(KeyCode::Space);
        } else {
            input.release(KeyCode::Space);
        }

        // Switch modes every 4 seconds
        if mode_timer.elapsed_secs() >= 8.0 {
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

    // prevent the bot from running too fast
    fn bot_wait(timeline: Res<LocalTimeline>) {
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(not(feature = "client"))]
mod bot {
    use bevy::prelude::*;

    #[derive(Component)]
    pub struct BotClient;
}

fn apply_replication_mode_cycle(
    replication_mode: &mut GameReplicationMode,
    rooms: &GameRooms,
    input_config: &mut ServerInputConfig<PlayerContext>,
    clients: &Query<Entity, With<ClientOf>>,
    players: &Query<Entity, With<PlayerMarker>>,
    commands: &mut Commands,
) {
    let current_mode = *replication_mode;
    *replication_mode = replication_mode.next();

    // only rebroadcast if clients predict other clients
    match *replication_mode {
        GameReplicationMode::AllPredicted | GameReplicationMode::OnlyInputsReplicated => {
            info!("Setting rebroadcast inputs to True");
            input_config.rebroadcast_inputs = true;
        }
        _ => {
            info!("Setting rebroadcast inputs to False");
            input_config.rebroadcast_inputs = false;
        }
    }

    // Move all clients from current room to next room
    if let (Some(current_room), Some(next_room)) = (
        rooms.rooms.get(&current_mode),
        rooms.rooms.get(&*replication_mode),
    ) {
        // also manually remove the Actions of the players present in the current room
        // (otherwise we might still be sending input messages for those actions even though the clients
        //  have despawned the corresponding player entities)
        for player in players.iter() {
            commands
                .entity(player)
                .despawn_related::<Actions<PlayerContext>>();
        }
        for client_entity in clients.iter() {
            commands
                .entity(client_entity)
                .insert(lightyear::prelude::Rooms::single(*next_room));
            info!(
                "Switching client {client_entity:?} from room {current_room:?} to room {next_room:?}"
            );
        }
    }

    info!("Cycled to replication mode: {}", replication_mode.name());
}
