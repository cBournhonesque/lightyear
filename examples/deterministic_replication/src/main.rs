#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use bevy::prelude::*;
use core::time::Duration;
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

/// how many ticks to delay the input by.
///
/// In deterministic replication, the input-delay must be large enough that
/// an input sent at local tick `T` arrives at all other peers before they
/// simulate tick `T`. Everyone simulates the same tick from the same input
/// state, so bad delay means the local peer applies input X at tick T
/// while every remote peer applies the default (or the previous decayed)
/// input at tick T — producing an immediate checksum divergence.
///
/// With the default `LinkConditionerConfig::average_condition` of
/// ~100 ms + 15 ms jitter, one-way latency is ~7-8 ticks at 64 Hz, so we
/// need at least that much delay before server-side inputs stay ahead of
/// the sim. 10 ticks gives some margin without being too sluggish.
pub const INPUT_DELAY_TICKS: u16 = 10;

mod automation;
#[cfg(feature = "client")]
mod client;
mod protocol;

#[cfg(feature = "gui")]
mod renderer;
#[cfg(feature = "server")]
mod server;
mod shared;

fn main() {
    let cli = Cli::default();

    let mut app = cli.build_app(Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ), true);

    app.add_plugins(SharedPlugin);
    cli.spawn_connections(&mut app);

    match cli.mode {
        #[cfg(feature = "client")]
        Some(Mode::Client { .. }) => {
            app.add_plugins(ExampleClientPlugin);
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
        app.add_plugins(renderer::ExampleRendererPlugin);
    }

    app.run();
}

#[cfg(feature = "client")]
fn add_input_delay(app: &mut App) {
    use lightyear::prelude::client::{InputDelayConfig, InputTimelineConfig};
    use lightyear::prelude::{Client, PredictionManager, RollbackMode, RollbackPolicy};
    let client = app
        .world_mut()
        .query_filtered::<Entity, With<Client>>()
        .single(app.world_mut())
        .unwrap();

    // set some input-delay since we are predicting all entities
    app.world_mut()
        .entity_mut(client)
        .insert(PredictionManager {
            rollback_policy: RollbackPolicy {
                state: RollbackMode::Disabled,
                // Deterministic replication relies entirely on input
                // rebroadcast: the server and every client simulate every
                // tick from the same inputs. If a remote input arrives
                // late (or its in-buffer prediction turns out wrong), the
                // client must re-run the sim from the first mismatched
                // tick forward so its state catches up to what the server
                // produces. Rolling back on the first stale input is what
                // keeps the checksum from drifting under real-world
                // latency/jitter.
                input: RollbackMode::Check,
                max_rollback_ticks: 100,
            },
            ..default()
        })
        .insert(
            InputTimelineConfig::default()
                // In deterministic mode, input delay must be large enough for
                // inputs to arrive on the server before the tick is simulated.
                .with_input_delay(InputDelayConfig::fixed_input_delay(INPUT_DELAY_TICKS)),
        );
}
