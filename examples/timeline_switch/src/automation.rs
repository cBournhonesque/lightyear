use bevy::prelude::*;
use lightyear_examples_common::automation::{env_string, sync_pressed_keys, HeadlessInputPlugin};

#[cfg(feature = "client")]
pub struct AutomationClientPlugin;

#[cfg(feature = "client")]
impl Plugin for AutomationClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(HeadlessInputPlugin);
        app.add_systems(Startup, client::init_settings);
        app.add_systems(First, client::drive_keys);
        app.add_systems(Update, crate::debug::client::mark_debug_entities);
    }
}

#[cfg(feature = "server")]
pub struct AutomationServerPlugin;

#[cfg(feature = "server")]
impl Plugin for AutomationServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, crate::debug::server::mark_debug_entities);
    }
}

#[cfg(feature = "client")]
mod client {
    use super::*;

    /// One step of a scripted input sequence: hold `pressed_keys` for `secs`.
    #[derive(Resource, Clone, Default)]
    pub(super) struct AutomationSettings {
        steps: Vec<AutomationStep>,
    }

    #[derive(Clone)]
    struct AutomationStep {
        pressed_keys: Vec<KeyCode>,
        secs: f32,
    }

    impl AutomationSettings {
        /// Parse `LIGHTYEAR_AUTOMOVE` as a comma-separated sequence of
        /// `key[*secs]` tokens, e.g. `W*5,E,NONE*4`. A bare key taps for
        /// 0.3s. `NONE` (or empty) releases everything for the given time.
        fn from_env() -> Self {
            let Some(value) = env_string("LIGHTYEAR_AUTOMOVE") else {
                return Self::default();
            };
            let mut steps = Vec::new();
            for token in value.split(',') {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                let (key_part, secs_part) = match token.split_once('*') {
                    Some((k, s)) => (k, s.parse::<f32>().ok()),
                    None => (token, None),
                };
                let Some(keys) = parse_keys(key_part) else {
                    warn!(token, "Ignoring unknown LIGHTYEAR_AUTOMOVE token");
                    continue;
                };
                steps.push(AutomationStep {
                    pressed_keys: keys,
                    secs: secs_part.unwrap_or(0.3),
                });
            }
            Self { steps }
        }
    }

    pub(super) fn init_settings(mut commands: Commands) {
        commands.insert_resource(AutomationSettings::from_env());
    }

    pub(super) fn drive_keys(
        settings: Res<AutomationSettings>,
        time: Res<Time>,
        mut state: Local<(usize, f32)>,
        mut previous: Local<Vec<KeyCode>>,
        mut buttons: ResMut<ButtonInput<KeyCode>>,
    ) {
        if settings.steps.is_empty() {
            return;
        }
        let (index, elapsed) = &mut *state;
        // Hold past the end of the script: release everything.
        let Some(step) = settings.steps.get(*index) else {
            sync_pressed_keys(&mut buttons, &mut previous, &[]);
            return;
        };
        *elapsed += time.delta_secs();
        if *elapsed >= step.secs {
            *index += 1;
            *elapsed = 0.0;
            return;
        }
        sync_pressed_keys(&mut buttons, &mut previous, &step.pressed_keys);
    }

    /// Maps one key token to key codes. Returns `Some(vec![])` for a pause.
    fn parse_keys(token: &str) -> Option<Vec<KeyCode>> {
        let mut keys = Vec::new();
        for part in token.split('+') {
            match part.trim().to_ascii_lowercase().as_str() {
                "up" | "u" | "forward" | "f" | "w" => keys.push(KeyCode::KeyW),
                "down" | "back" | "b" | "s" => keys.push(KeyCode::KeyS),
                "left" | "l" | "a" => keys.push(KeyCode::KeyA),
                "right" | "r" | "d" => keys.push(KeyCode::KeyD),
                "jump" | "space" => keys.push(KeyCode::Space),
                "pickup" | "e" => keys.push(KeyCode::KeyE),
                "" | "none" => {}
                _ => return None,
            }
        }
        Some(keys)
    }
}
