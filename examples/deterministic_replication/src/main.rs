#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "p2p")]
use crate::p2p::ExampleP2PPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
#[cfg(feature = "client")]
use bevy::prelude::*;
use core::time::Duration;
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

/// Default number of ticks to delay local input by.
pub const DEFAULT_INPUT_DELAY_TICKS: u16 = 0;
/// Default fixed input-scheduling safety margin, in ticks.
///
/// Deterministic replication requires the server to receive a client's input
/// for tick `T` before the server simulates `T`. This margin covers normal
/// fixed-frame batching, where the client may only send once after running
/// several fixed ticks in one render frame.
pub const DEFAULT_INPUT_SYNC_MARGIN_TICKS: f32 = 3.0;

mod automation;
#[cfg(feature = "client")]
mod client;
mod debug;
#[cfg(feature = "p2p")]
mod p2p;
mod protocol;

#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;
mod shared;

fn main() {
    let cli = Cli::default();
    let headless = cli.headless();

    let mut app = cli.build_app(Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ), true);

    app.add_plugins(SharedPlugin);
    cli.spawn_connections(&mut app);

    match cli.mode {
        #[cfg(all(feature = "client", any(not(feature = "p2p"), feature = "netcode")))]
        Some(Mode::Client { .. }) => {
            app.add_plugins(ExampleClientPlugin);
            enable_prediction(&mut app);
            add_input_delay(&mut app);
        }
        #[cfg(feature = "p2p")]
        Some(Mode::P2P { .. }) => {
            app.add_plugins((ExampleClientPlugin, ExampleP2PPlugin));
            enable_prediction(&mut app);
            add_input_delay(&mut app);
        }
        #[cfg(feature = "server")]
        Some(Mode::Server) => {
            app.add_plugins(ExampleServerPlugin);
        }
        #[cfg(all(feature = "client", feature = "server"))]
        Some(Mode::HostClient { client_id }) => {
            app.add_plugins(ExampleClientPlugin);
            app.add_plugins(ExampleServerPlugin);
            add_input_delay(&mut app);
        }
        _ => {}
    }

    #[cfg(feature = "gui")]
    {
        if !headless {
            app.add_plugins(renderer::ExampleRendererPlugin);
        }
    }

    app.run();
}

#[cfg(feature = "client")]
fn enable_prediction(app: &mut App) {
    use lightyear::prelude::{PredictionManager, RollbackMode, RollbackPolicy};

    app.insert_resource(PredictionManager {
        rollback_policy: RollbackPolicy {
            state: RollbackMode::Disabled,
            input: RollbackMode::Check,
            max_rollback_ticks: 100,
        },
        ..default()
    });
}

#[cfg(feature = "client")]
fn add_input_delay(app: &mut App) {
    use lightyear::prelude::SyncConfig;
    use lightyear::prelude::client::{InputDelayConfig, InputTimelineConfig};

    // Set some input delay since a remote client predicts all deterministic entities. The
    // host-client uses the same delayed input scheduling but does not enable rollback for its
    // authoritative in-process simulation.
    app.insert_resource(
        InputTimelineConfig::default()
            .with_sync_config(SyncConfig {
                jitter_margin: input_sync_margin_ticks(),
                ..default()
            })
            .with_input_delay(InputDelayConfig::fixed_input_delay(input_delay_ticks())),
    );
}

#[cfg(feature = "client")]
fn input_delay_ticks() -> u16 {
    #[cfg(not(target_family = "wasm"))]
    {
        std::env::var("LIGHTYEAR_INPUT_DELAY_TICKS")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_INPUT_DELAY_TICKS)
    }
    #[cfg(target_family = "wasm")]
    {
        DEFAULT_INPUT_DELAY_TICKS
    }
}

#[cfg(feature = "client")]
fn input_sync_margin_ticks() -> f32 {
    #[cfg(not(target_family = "wasm"))]
    {
        std::env::var("LIGHTYEAR_INPUT_SYNC_MARGIN_TICKS")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(DEFAULT_INPUT_SYNC_MARGIN_TICKS)
    }
    #[cfg(target_family = "wasm")]
    {
        DEFAULT_INPUT_SYNC_MARGIN_TICKS
    }
}
