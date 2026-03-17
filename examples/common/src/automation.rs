use bevy::input::InputPlugin;
use bevy::prelude::*;

pub fn env_flag(name: &str) -> bool {
    #[cfg(not(target_family = "wasm"))]
    {
        std::env::var(name)
            .map(|value| value != "0")
            .unwrap_or(false)
    }
    #[cfg(target_family = "wasm")]
    {
        false
    }
}

pub fn env_string(name: &str) -> Option<String> {
    #[cfg(not(target_family = "wasm"))]
    {
        std::env::var(name).ok()
    }
    #[cfg(target_family = "wasm")]
    {
        None
    }
}

pub fn sync_pressed_keys(
    buttons: &mut ButtonInput<KeyCode>,
    previous: &mut Vec<KeyCode>,
    next: &[KeyCode],
) {
    for key in previous.iter().copied() {
        if !next.contains(&key) {
            buttons.release(key);
        }
    }
    for key in next.iter().copied() {
        buttons.press(key);
    }
    *previous = next.to_vec();
}

pub struct HeadlessInputPlugin;

impl Plugin for HeadlessInputPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<ButtonInput<KeyCode>>() {
            app.add_plugins(InputPlugin);
        }
    }
}
