use crate::client::connection::ConnectionManager;
use crate::client::prediction::diagnostics::PredictionDiagnosticsPlugin;
use bevy::app::{App, Plugin, PostUpdate};
use bevy::diagnostic::Diagnostics;
use bevy::prelude::{not, Condition, IntoScheduleConfigs, Real, Res, ResMut, Time};
use bevy::time::common_conditions::on_timer;
use core::time::Duration;

use crate::connection::client::{ClientConnection, NetClient};
use crate::prelude::{client::is_disconnected, is_host_server};
use crate::shared::ping::diagnostics::PingDiagnosticsPlugin;
use crate::transport::io::IoDiagnosticsPlugin;

// TODO: ideally make this a plugin group? but nested plugin groups are not supported
#[derive(Debug)]
pub struct ClientDiagnosticsPlugin {
    flush_interval: Duration,
}

impl Default for ClientDiagnosticsPlugin {
    fn default() -> Self {
        Self {
            flush_interval: Duration::from_millis(200),
        }
    }
}

fn io_diagnostics_system(
    mut netclient: ResMut<ClientConnection>,
    time: Res<Time<Real>>,
    mut diagnostics: Diagnostics,
) {
    if let Some(io) = netclient.io_mut() {
        IoDiagnosticsPlugin::update_diagnostics(&mut io.stats, &time, &mut diagnostics);
    }
}

fn ping_diagnostics_system(connection: Res<ConnectionManager>, diagnostics: Diagnostics) {
    PingDiagnosticsPlugin::add_measurements(&connection.ping_manager, diagnostics);
}

impl Plugin for ClientDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        {
            let ping_plugin = PingDiagnosticsPlugin::default();
            let flush_interval = ping_plugin.flush_interval;
            app.add_plugins(ping_plugin);
            app.add_systems(
                PostUpdate,
                ping_diagnostics_system
                    .run_if(on_timer(flush_interval).and(not(is_host_server.or(is_disconnected)))),
            );
        }
        app.add_plugins(PredictionDiagnosticsPlugin::default());

        {
            app.add_plugins(IoDiagnosticsPlugin);
            app.add_systems(
                PostUpdate,
                io_diagnostics_system.run_if(
                    // on_timer(self.flush_interval).and(
                    not(is_host_server.or(is_disconnected)),
                ),
            );
        }
    }
}
