extern crate alloc;

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
#[cfg(feature = "client")]
use lightyear::netcode::NetcodeClient;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::{
    LagCompensationHistory, LagCompensationPlugin, LagCompensationSpatialQuery,
};
use lightyear_examples_common::shared::{SEND_INTERVAL, SERVER_ADDR, SHARED_SETTINGS};

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

        app.add_systems(
            Startup,
            (spawn_global_control, apply_initial_input_config).chain(),
        );
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
    commands
        .entity(trigger.entity)
        .insert((ReplicationSender, Name::from("ClientOf")));
}

pub(crate) fn spawn_global_control(mut commands: Commands) {
    let replication_mode = initial_replication_mode();
    let projectile_mode = initial_projectile_mode();
    let weapon_type = initial_weapon_type();
    info!(
        replication_mode = replication_mode.name(),
        projectile_mode = projectile_mode.name(),
        weapon_type = weapon_type.name(),
        "Starting projectiles example modes"
    );
    let global = commands
        .spawn((
            ClientContext,
            Replicate::to_clients(NetworkTarget::All),
            replication_mode,
            projectile_mode,
            weapon_type,
            Name::new("ClientContext"),
        ))
        .id();
    shared::spawn_global_actions(&mut commands, global);
}

fn apply_initial_input_config(
    global: Single<&GameReplicationMode, With<ClientContext>>,
    mut input_config: ResMut<ServerInputConfig<PlayerContext>>,
) {
    input_config.rebroadcast_inputs = matches!(
        *global.into_inner(),
        GameReplicationMode::AllPredicted | GameReplicationMode::OnlyInputsReplicated
    );
}

fn initial_replication_mode() -> GameReplicationMode {
    let Some(value) = std::env::var("LIGHTYEAR_INITIAL_REPLICATION_MODE").ok() else {
        return GameReplicationMode::default();
    };
    match normalized_env(&value).as_str() {
        "0" | "allpredicted" | "all_predicted" => GameReplicationMode::AllPredicted,
        "1" | "clientpredictednocomp" | "client_predicted_no_comp" | "no_comp" => {
            GameReplicationMode::ClientPredictedNoComp
        }
        "2" | "clientpredictedlagcomp" | "client_predicted_lag_comp" | "lag_comp" => {
            GameReplicationMode::ClientPredictedLagComp
        }
        "3" | "clientsidehitdetection" | "client_side_hit_detection" | "client_side" => {
            GameReplicationMode::ClientSideHitDetection
        }
        "4" | "allinterpolated" | "all_interpolated" => GameReplicationMode::AllInterpolated,
        "5" | "onlyinputsreplicated" | "only_inputs_replicated" | "inputs_only" => {
            GameReplicationMode::OnlyInputsReplicated
        }
        other => {
            warn!(
                value = other,
                "Ignoring unknown LIGHTYEAR_INITIAL_REPLICATION_MODE"
            );
            GameReplicationMode::default()
        }
    }
}

fn initial_projectile_mode() -> ProjectileReplicationMode {
    let Some(value) = std::env::var("LIGHTYEAR_INITIAL_PROJECTILE_MODE").ok() else {
        return ProjectileReplicationMode::default();
    };
    match normalized_env(&value).as_str() {
        "full" | "fullentity" | "full_entity" | "0" => ProjectileReplicationMode::FullEntity,
        "direction" | "directiononly" | "direction_only" | "1" => {
            ProjectileReplicationMode::DirectionOnly
        }
        other => {
            warn!(
                value = other,
                "Ignoring unknown LIGHTYEAR_INITIAL_PROJECTILE_MODE"
            );
            ProjectileReplicationMode::default()
        }
    }
}

fn initial_weapon_type() -> WeaponType {
    let Some(value) = std::env::var("LIGHTYEAR_INITIAL_WEAPON").ok() else {
        return WeaponType::default();
    };
    match normalized_env(&value).as_str() {
        "hitscan" | "hit_scan" | "0" => WeaponType::Hitscan,
        "bullet" | "linear" | "linearprojectile" | "linear_projectile" | "1" => WeaponType::Bullet,
        other => {
            warn!(value = other, "Ignoring unknown LIGHTYEAR_INITIAL_WEAPON");
            WeaponType::default()
        }
    }
}

fn normalized_env(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', ' '], "_")
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
    active_mode: Single<&GameReplicationMode, With<ClientContext>>,
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

    let active_mode = *active_mode.into_inner();
    for i in 0..6 {
        let replication_mode = GameReplicationMode::from_room_id(i);
        let room_id = *rooms
            .rooms
            .entry(replication_mode)
            .or_insert_with(|| room_allocator.allocate());

        // start by adding the player to the first room
        if replication_mode == active_mode {
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
        shared::spawn_player_actions(&mut commands, player_entity, room_id);
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
    use alloc::sync::Arc;
    use bevy::app::{AppExit, PluginsState};
    use core::sync::atomic::{AtomicU32, Ordering};
    use lightyear::prelude::client::{InputDelayConfig, InputTimelineConfig};
    use std::time::Instant;

    pub struct BotPlugin;

    impl Plugin for BotPlugin {
        fn build(&self, app: &mut App) {}

        // run in `cleanup` because BEI finishes building in `finish`
        fn cleanup(&self, app: &mut App) {
            app.add_observer(spawn_bot_app);
            app.add_systems(Startup, spawn_bots);
            app.add_systems(Last, update_bot_server_ticks);
        }
    }

    #[derive(Component)]
    pub struct BotClient;

    #[derive(Clone, Component)]
    struct BotServerTick(Arc<AtomicU32>);

    const BOT_CLIENT_ID: u64 = 10_000;
    const BOT_MAX_TICK_AHEAD: u32 = 8;
    const BOT_INPUT_DELAY_TICKS: u16 = 12;

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

    /// Spawn bots after `Plugin::finish()` so BEI has finished building.
    pub(crate) fn spawn_bots(mut commands: Commands) {
        commands.trigger(SpawnBot);
    }

    /// On the server, we will create a second app to host a bot that is similar to a real client,
    /// but their inputs are mocked
    fn spawn_bot_app(
        trigger: On<SpawnBot>,
        tick_duration: Res<TickDuration>,
        timeline: Res<LocalTimeline>,
        server: Single<Entity, With<Server>>,
        mut commands: Commands,
    ) {
        info!("Spawning bot app");
        let (crossbeam_client, crossbeam_server) = CrossbeamIo::new_pair();
        let server_tick = Arc::new(AtomicU32::new(timeline.tick().0));
        let bot_runner_server_tick = server_tick.clone();

        // Bots are client apps; pace their main loop to ~60 FPS so they
        // don't flood the server with packets under the
        // MinimalPlugins-default run-as-fast-as-possible scheduler.
        let loop_wait =
            Duration::from_secs_f64(1.0 / lightyear_examples_common::cli::HEADLESS_CLIENT_LOOP_HZ);
        let mut app = new_bot_headless_app(loop_wait);
        app.set_runner(move |app| run_bot_app(app, bot_runner_server_tick, loop_wait));
        app.add_plugins(lightyear::prelude::client::ClientPlugins {
            tick_duration: tick_duration.0,
        });
        app.add_plugins(SharedPlugin);
        app.add_plugins(ExampleClientPlugin);

        let client_id = BOT_CLIENT_ID;
        let auth = Authentication::Manual {
            server_addr: SERVER_ADDR,
            client_id,
            private_key: SHARED_SETTINGS.private_key,
            protocol_id: SHARED_SETTINGS.protocol_id,
        };
        let conditioner = LinkConditionerConfig::average_condition().half();

        app.world_mut().spawn((
            Client,
            BotClient,
            ReplicationSender,
            ReplicationReceiver,
            Link::new(Some(RecvLinkConditioner::new(conditioner.clone()))),
            NetcodeClient::new(
                auth,
                lightyear::netcode::client_plugin::NetcodeConfig::default(),
            )
            .unwrap(),
            crossbeam_client,
            PredictionManager::default(),
            InputTimelineConfig::default()
                .with_input_delay(InputDelayConfig::fixed_input_delay(BOT_INPUT_DELAY_TICKS)),
            Name::from("BotClient"),
        ));
        let server = server.into_inner();
        commands.spawn((
            LinkOf { server },
            Link::new(Some(RecvLinkConditioner::new(conditioner))),
            Linked,
            ClientOf,
            BotClient,
            BotServerTick(server_tick),
            crossbeam_server,
            ReplicationSender,
        ));

        app.add_systems(Startup, bot_connect);
        app.add_systems(FixedFirst, bot_inputs.run_if(not(is_in_rollback)));
        let mut bot_app = BotApp(app);
        std::thread::spawn(move || {
            bot_app.run();
        });
    }

    fn update_bot_server_ticks(timeline: Res<LocalTimeline>, pacers: Query<&BotServerTick>) {
        for pacer in &pacers {
            pacer.0.store(timeline.tick().0, Ordering::Relaxed);
        }
    }

    fn bot_connect(bot: Single<Entity, (With<BotClient>, With<Client>)>, mut commands: Commands) {
        let entity = bot.into_inner();
        info!("Bot entity {entity:?} connecting to server");
        commands.trigger(Connect { entity });
    }

    fn run_bot_app(mut app: App, server_tick: Arc<AtomicU32>, loop_wait: Duration) -> AppExit {
        while app.plugins_state() == PluginsState::Adding {
            std::thread::yield_now();
        }
        app.finish();
        app.cleanup();

        loop {
            let start = Instant::now();
            app.update();

            if let Some(exit) = app.should_exit() {
                return exit;
            }

            wait_until_bot_is_not_too_far_ahead(&app, &server_tick);

            let elapsed = start.elapsed();
            if elapsed < loop_wait {
                std::thread::sleep(loop_wait - elapsed);
            }
        }
    }

    fn wait_until_bot_is_not_too_far_ahead(app: &App, server_tick: &AtomicU32) {
        let Some(timeline) = app.world().get_resource::<LocalTimeline>() else {
            return;
        };
        let bot_tick = timeline.tick().0;
        while bot_tick
            > server_tick
                .load(Ordering::Relaxed)
                .saturating_add(BOT_MAX_TICK_AHEAD)
        {
            std::thread::sleep(Duration::from_millis(5));
        }
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
        global_mode: Single<&GameReplicationMode, With<ClientContext>>,
        players: Query<(&Position, &GameReplicationMode), (With<Controlled>, With<PlayerMarker>)>,
        mut local: Local<BotLocal>,
    ) {
        let active_mode = *global_mode.into_inner();
        let Some((position, _)) = players.iter().find(|(_, mode)| **mode == active_mode) else {
            return;
        };
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
        let pos_x = position.x;
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

    fn new_bot_headless_app(loop_wait: Duration) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(loop_wait)),
            TransformPlugin,
            bevy::input::InputPlugin,
            bevy::state::app::StatesPlugin,
            bevy::diagnostic::DiagnosticsPlugin,
        ));
        app
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
