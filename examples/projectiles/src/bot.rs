//! Embedded headless client used by the projectiles example.
//!
//! The bot is intentionally a real Lightyear client running in its own Bevy
//! `App`. It connects to the server through crossbeam channels, receives the
//! same replicated entities as an external client, and produces normal BEI
//! inputs. Keeping it here leaves `server.rs` focused on server authority and
//! arena lifecycle.

use bevy::prelude::*;

/// Marks both ends of the embedded bot's local crossbeam connection.
///
/// This type exists in server-only builds too because arena spawning uses
/// `Has<BotClient>` to decide whether the connected peer should get the
/// replicated `Bot` presentation marker.
#[derive(Component)]
pub(crate) struct BotClient;

#[cfg(feature = "client")]
mod embedded {
    use super::*;
    use crate::client::ExampleClientPlugin;
    use crate::protocol::PlayerMarker;
    use crate::shared::SharedPlugin;
    use avian2d::prelude::Position;
    use bevy::app::{AppExit, PluginsState};
    use bevy::platform::sync::Arc;
    use bevy::time::Stopwatch;
    use core::ops::DerefMut;
    use core::sync::atomic::{AtomicU32, Ordering};
    use core::time::Duration;
    use lightyear::core::tick::TickDuration;
    use lightyear::crossbeam::CrossbeamIo;
    use lightyear::netcode::NetcodeClient;
    use lightyear::prelude::client::{InputDelayConfig, InputTimelineConfig};
    use lightyear::prelude::server::*;
    use lightyear::prelude::*;
    use lightyear_examples_common::shared::{SERVER_ADDR, SHARED_SETTINGS};
    use std::time::Instant;

    pub(crate) struct BotPlugin;

    impl Plugin for BotPlugin {
        fn build(&self, _app: &mut App) {}

        // BEI finishes building its action registry in `Plugin::finish`, so
        // create the nested client only once the outer app reaches cleanup.
        fn cleanup(&self, app: &mut App) {
            app.add_observer(spawn_bot_app);
            app.add_systems(Startup, spawn_bot);
            app.add_systems(Last, update_bot_server_ticks);
        }
    }

    #[derive(Clone, Component)]
    struct BotServerTick(Arc<AtomicU32>);

    const BOT_CLIENT_ID: u64 = 10_000;
    const BOT_MAX_TICK_AHEAD: u32 = 8;
    const BOT_INPUT_DELAY_TICKS: u16 = 12;

    #[derive(Event)]
    struct SpawnBot;

    struct BotApp(App);

    // The nested App stays on its dedicated thread for its entire lifetime.
    // These impls allow the outer Bevy world to own it until that thread starts.
    unsafe impl Send for BotApp {}
    unsafe impl Sync for BotApp {}

    impl BotApp {
        fn run(&mut self) {
            info_span!("bot").in_scope(|| self.0.run());
        }
    }

    fn spawn_bot(mut commands: Commands) {
        commands.trigger(SpawnBot);
    }

    fn spawn_bot_app(
        _trigger: On<SpawnBot>,
        tick_duration: Res<TickDuration>,
        timeline: Res<LocalTimeline>,
        server: Single<Entity, With<Server>>,
        mut commands: Commands,
    ) {
        let (crossbeam_client, crossbeam_server) = CrossbeamIo::new_pair();
        let server_tick = Arc::new(AtomicU32::new(timeline.tick().0));
        let bot_runner_server_tick = server_tick.clone();

        let loop_wait =
            Duration::from_secs_f64(1.0 / lightyear_examples_common::cli::HEADLESS_CLIENT_LOOP_HZ);
        let mut app = new_bot_headless_app(loop_wait);
        app.set_runner(move |app| run_bot_app(app, bot_runner_server_tick, loop_wait));
        app.add_plugins(lightyear::prelude::client::ClientPlugins {
            tick_duration: tick_duration.0,
        });
        app.add_plugins(SharedPlugin);
        app.add_plugins(ExampleClientPlugin);
        app.insert_resource(
            InputTimelineConfig::default()
                .with_input_delay(InputDelayConfig::fixed_input_delay(BOT_INPUT_DELAY_TICKS)),
        );
        app.insert_resource(PredictionManager::default());

        let auth = Authentication::Manual {
            server_addr: SERVER_ADDR,
            client_id: BOT_CLIENT_ID,
            private_key: SHARED_SETTINGS.private_key,
            protocol_id: SHARED_SETTINGS.protocol_id,
        };
        let conditioner = LinkConditionerConfig::average_condition().half();
        app.world_mut().spawn((
            Client,
            BotClient,
            ReplicationSender,
            ReplicationReceiver,
            Link::default().with_conditioner(RecvLinkConditioner::new(conditioner.clone())),
            NetcodeClient::new(
                auth,
                lightyear::netcode::client_plugin::NetcodeConfig::default(),
            )
            .unwrap(),
            crossbeam_client,
            Name::from("BotClient"),
        ));

        // This is the server end of the same in-memory connection. It behaves
        // like every other ClientOf link after the netcode handshake completes.
        commands.spawn((
            LinkOf { server: *server },
            Link::default().with_conditioner(RecvLinkConditioner::new(conditioner)),
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
        std::thread::spawn(move || bot_app.run());
    }

    fn update_bot_server_ticks(timeline: Res<LocalTimeline>, pacers: Query<&BotServerTick>) {
        for pacer in &pacers {
            pacer.0.store(timeline.tick().0, Ordering::Relaxed);
        }
    }

    fn bot_connect(bot: Single<Entity, (With<BotClient>, With<Client>)>, mut commands: Commands) {
        commands.trigger(Connect { entity: *bot });
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
            if let Some(remaining) = loop_wait.checked_sub(start.elapsed()) {
                std::thread::sleep(remaining);
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
        Strafing,
        StraightLine,
    }

    impl BotMovementMode {
        fn interval(self) -> f32 {
            match self {
                Self::Strafing => 0.4,
                Self::StraightLine => 2.0,
            }
        }
    }

    struct BotLocal {
        mode_timer: Stopwatch,
        key_timer: Stopwatch,
        shoot_timer: Timer,
        current_mode: BotMovementMode,
        press_a: bool,
        override_direction: Option<bool>,
    }

    impl Default for BotLocal {
        fn default() -> Self {
            Self {
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
        players: Query<&Position, (With<Controlled>, With<PlayerMarker>)>,
        mut local: Local<BotLocal>,
    ) {
        let Some(position) = players.iter().next() else {
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

        // Reverse before the bot leaves the useful part of the test arena.
        let threshold = 500.0;
        if position.x.abs() > threshold {
            *override_direction = Some(position.x > 0.0);
        } else if override_direction.is_some() && position.x.abs() <= threshold * 0.8 {
            *override_direction = None;
        }

        mode_timer.tick(time.delta());
        key_timer.tick(time.delta());
        shoot_timer.tick(time.delta());
        if shoot_timer.is_finished() {
            input.press(KeyCode::Space);
        } else {
            input.release(KeyCode::Space);
        }

        if mode_timer.elapsed_secs() >= 8.0 {
            mode_timer.reset();
            *current_mode = match *current_mode {
                BotMovementMode::Strafing => BotMovementMode::StraightLine,
                BotMovementMode::StraightLine => BotMovementMode::Strafing,
            };
        }
        if key_timer.elapsed_secs() >= current_mode.interval() {
            key_timer.reset();
            *press_a = !*press_a;
        }

        let press_a_now = override_direction.unwrap_or(*press_a);
        if press_a_now {
            input.press(KeyCode::KeyA);
            input.release(KeyCode::KeyD);
        } else {
            input.press(KeyCode::KeyD);
            input.release(KeyCode::KeyA);
        }
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

#[cfg(feature = "client")]
pub(crate) use embedded::BotPlugin;
