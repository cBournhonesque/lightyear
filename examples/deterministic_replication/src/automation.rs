use avian2d::prelude::Position;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::{ActionState, InputMap};
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear::prelude::*;
use lightyear_deterministic_replication::prelude::{CatchUpGated, CatchUpMode};
use lightyear_examples_common::automation::{HeadlessInputPlugin, env_string, sync_pressed_keys};

use crate::protocol::{PlayerActions, PlayerActivationTick, PlayerId};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_keys);
        app.add_systems(
            PreUpdate,
            client::drive_action_state.in_set(InputManagerSystem::ManualControl),
        );
        app.add_systems(Update, client::mark_debug_players);
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, server::mark_debug_players);
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    /// Randomized pattern: every `switch_interval` wall-time, pick
    /// a new random combination of 1-2 directional keys. Seeded from
    /// LIGHTYEAR_AUTOMOVE_SEED so different clients drive different
    /// patterns.
    #[derive(Clone)]
    pub(super) struct RandomDrive {
        seed: u64,
        switch_interval: Duration,
    }

    #[derive(Resource, Clone)]
    pub(super) struct AutomationSettings {
        pressed_keys: Vec<KeyCode>,
        random: Option<RandomDrive>,
        min_players_before_move: usize,
    }

    impl Default for AutomationSettings {
        fn default() -> Self {
            Self {
                pressed_keys: Vec::new(),
                random: None,
                min_players_before_move: 1,
            }
        }
    }

    impl AutomationSettings {
        fn from_env() -> Self {
            let random = env_string("LIGHTYEAR_AUTOMOVE_RANDOM")
                .map(|value| value != "0" && !value.is_empty())
                .unwrap_or(false);
            let seed = env_string("LIGHTYEAR_AUTOMOVE_SEED")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let switch_ms = env_string("LIGHTYEAR_AUTOMOVE_SWITCH_MS")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(500);
            let random = if random {
                Some(RandomDrive {
                    seed,
                    switch_interval: Duration::from_millis(switch_ms),
                })
            } else {
                None
            };
            Self {
                pressed_keys: parse_move_keys(env_string("LIGHTYEAR_AUTOMOVE")),
                random,
                min_players_before_move: env_string("LIGHTYEAR_AUTOMOVE_MIN_PLAYERS")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1),
            }
        }
    }

    #[derive(Resource, Default)]
    pub(super) struct RandomState {
        next_switch: Duration,
        current: Vec<KeyCode>,
        rng: u64,
    }

    pub(super) fn init_settings(mut commands: Commands) {
        let settings = AutomationSettings::from_env();
        if let Some(rand) = &settings.random {
            commands.insert_resource(RandomState {
                next_switch: Duration::ZERO,
                current: Vec::new(),
                rng: rand.seed.wrapping_add(0x9E3779B97F4A7C15),
            });
        }
        commands.insert_resource(settings);
    }

    fn splitmix64(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// Pick a new set of 1 or 2 keys pseudorandomly.
    fn next_random_keys(state: &mut RandomState) -> Vec<KeyCode> {
        const KEYS: [KeyCode; 4] = [KeyCode::KeyW, KeyCode::KeyS, KeyCode::KeyA, KeyCode::KeyD];
        let r = splitmix64(&mut state.rng);
        let count = 1 + ((r & 1) as usize);
        let first = ((r >> 1) % 4) as usize;
        let mut keys = Vec::with_capacity(2);
        keys.push(KEYS[first]);
        if count == 2 {
            let r2 = splitmix64(&mut state.rng);
            let mut second = (r2 % 4) as usize;
            if second == first {
                second = (second + 1) % 4;
            }
            let other = KEYS[second];
            // Skip opposite-direction pair which would cancel out visibly
            // but that's fine — we still want random inputs.
            keys.push(other);
        }
        keys
    }

    pub(super) fn drive_keys(
        time: Res<Time>,
        settings: Res<AutomationSettings>,
        random_state: Option<ResMut<RandomState>>,
        mode: Res<CatchUpMode>,
        timeline: Res<LocalTimeline>,
        client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
        players: Query<(
            &PlayerId,
            Option<&LeafwingBuffer<PlayerActions>>,
            Option<&PlayerActivationTick>,
            Has<CatchUpGated>,
        )>,
        mut previous: Local<Vec<KeyCode>>,
        mut ready_logged: Local<bool>,
        mut buttons: ResMut<ButtonInput<KeyCode>>,
    ) {
        if !input_rebroadcast_ready(
            &mode,
            timeline.tick(),
            client.as_deref().copied(),
            &players,
            &settings,
        ) {
            if *ready_logged {
                info!("automation paused until input rebroadcast is ready");
                *ready_logged = false;
            }
            sync_pressed_keys(&mut buttons, &mut previous, &[]);
            return;
        }
        if !*ready_logged {
            info!("automation input rebroadcast ready; starting movement");
            *ready_logged = true;
        }
        if let (Some(rand), Some(mut state)) = (settings.random.as_ref(), random_state) {
            let now = time.elapsed();
            if now >= state.next_switch {
                state.current = next_random_keys(&mut state);
                state.next_switch = now + rand.switch_interval;
                info!(keys = ?state.current, ?now, "random automation switch");
            }
            sync_pressed_keys(&mut buttons, &mut previous, &state.current);
        } else {
            sync_pressed_keys(&mut buttons, &mut previous, &settings.pressed_keys);
        }
    }

    pub(super) fn drive_action_state(
        settings: Res<AutomationSettings>,
        random_state: Option<Res<RandomState>>,
        mode: Res<CatchUpMode>,
        timeline: Res<LocalTimeline>,
        client: Option<Single<&LocalId, (With<Client>, With<IsSynced<InputTimeline>>)>>,
        players: Query<(
            &PlayerId,
            Option<&LeafwingBuffer<PlayerActions>>,
            Option<&PlayerActivationTick>,
            Has<CatchUpGated>,
        )>,
        mut query: Query<&mut ActionState<PlayerActions>, With<InputMap<PlayerActions>>>,
    ) {
        if !input_rebroadcast_ready(
            &mode,
            timeline.tick(),
            client.as_deref().copied(),
            &players,
            &settings,
        ) {
            for mut action_state in &mut query {
                release_all(&mut action_state);
            }
            return;
        }
        let keys: &[KeyCode] = if settings.random.is_some() {
            random_state
                .as_ref()
                .map(|s| s.current.as_slice())
                .unwrap_or(&[])
        } else {
            settings.pressed_keys.as_slice()
        };
        for mut action_state in &mut query {
            release_all(&mut action_state);
            for key in keys {
                match key {
                    KeyCode::KeyW => action_state.press(&PlayerActions::Up),
                    KeyCode::KeyS => action_state.press(&PlayerActions::Down),
                    KeyCode::KeyA => action_state.press(&PlayerActions::Left),
                    KeyCode::KeyD => action_state.press(&PlayerActions::Right),
                    _ => {}
                }
            }
        }
    }

    fn input_rebroadcast_ready(
        mode: &CatchUpMode,
        local_tick: Tick,
        client: Option<&LocalId>,
        players: &Query<(
            &PlayerId,
            Option<&LeafwingBuffer<PlayerActions>>,
            Option<&PlayerActivationTick>,
            Has<CatchUpGated>,
        )>,
        settings: &AutomationSettings,
    ) -> bool {
        let Some(local_id) = client else {
            return false;
        };
        let mut count = 0;
        let mut local_can_move = false;
        for (player_id, buffer, activation_tick, awaiting_catchup) in players.iter() {
            count += 1;
            if player_id.0 == local_id.0 {
                if awaiting_catchup {
                    return false;
                }
                let Some(buffer) = buffer else {
                    return false;
                };
                if buffer.start_tick.is_none() {
                    return false;
                }
                let Some(activation_tick) = activation_tick else {
                    return false;
                };
                local_can_move = !activation_tick.is_pending() && local_tick >= activation_tick.0;
            } else if *mode == CatchUpMode::InputOnly {
                let Some(buffer) = buffer else {
                    return false;
                };
                if buffer.last_remote_tick.is_none() {
                    return false;
                }
            }
        }
        count >= settings.min_players_before_move && local_can_move
    }

    fn release_all(action_state: &mut ActionState<PlayerActions>) {
        for action in [
            PlayerActions::Up,
            PlayerActions::Down,
            PlayerActions::Left,
            PlayerActions::Right,
        ] {
            action_state.release(&action);
        }
    }

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        query: Query<Entity, (Added<PlayerId>, With<Position>)>,
    ) {
        for entity in &query {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    fn parse_move_keys(value: Option<String>) -> Vec<KeyCode> {
        let mut keys = Vec::new();
        let Some(value) = value else {
            return keys;
        };
        for token in value.split(',') {
            match token.trim().to_ascii_lowercase().as_str() {
                "up" | "u" => keys.push(KeyCode::KeyW),
                "down" | "d" => keys.push(KeyCode::KeyS),
                "left" | "l" => keys.push(KeyCode::KeyA),
                "right" | "r" => keys.push(KeyCode::KeyD),
                "" | "none" => {}
                other => warn!(token = other, "Ignoring unknown LIGHTYEAR_AUTOMOVE token"),
            }
        }
        keys
    }
}

#[cfg(feature = "server")]
mod server {
    use super::*;

    pub(super) fn mark_debug_players(
        mut commands: Commands,
        query: Query<Entity, (Added<PlayerId>, With<Position>)>,
    ) {
        for entity in &query {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::Update,
                ]));
        }
    }
}
