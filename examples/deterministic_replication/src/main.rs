#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]
#[cfg(feature = "client")]
use crate::client::ExampleClientPlugin;
#[cfg(feature = "server")]
use crate::server::ExampleServerPlugin;
use crate::shared::SharedPlugin;
use avian2d::position::Position;
use bevy::prelude::*;
use core::time::Duration;
use lightyear::prelude::*;
use lightyear_examples_common::cli::{Cli, Mode};
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

/// how many ticks to delay the input by
pub const INPUT_DELAY_TICKS: u16 = 0;

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
        // app.add_plugins(bevy_metrics_dashboard::RegistryPlugin::default())
        //     .add_plugins(bevy_metrics_dashboard::DashboardPlugin);
        // app.world_mut()
        //     .spawn(bevy_metrics_dashboard::DashboardWindow::new(
        //         "Metrics Dashboard",
        //     ));
    }

    app.run();
}

#[cfg(feature = "client")]
fn add_input_delay(app: &mut App) {
    use lightyear::prelude::client::{Input, InputDelayConfig};
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
                // we only replicate inputs, so state-based rollback is disabled
                state: RollbackMode::Disabled,
                // we rollback only when remote inputs don't match what we were predicting
                input: RollbackMode::Check,
                // do not limit the max number of rollback ticks
                max_rollback_ticks: 100,
            },
            ..default()
        })
        .insert(InputTimeline(Timeline::from(
            Input::default()
                // Enable `no_prediction()` to do deterministic_lockstep! 100% of the latency will be covered
                // by input delay so there won't be any rollbacks
                // .with_input_delay(InputDelayConfig::no_prediction()),
                // Otherwise control the input delay manually
                .with_input_delay(InputDelayConfig::fixed_input_delay(INPUT_DELAY_TICKS)),
        )));
}
