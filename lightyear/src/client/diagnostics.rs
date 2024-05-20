use bevy::app::{App, Plugin, PostUpdate};
use bevy::diagnostic::Diagnostics;
use bevy::prelude::{not, Condition, IntoSystemConfigs, Real, Res, ResMut, Time};

use crate::connection::client::{ClientConnection, NetClient};
use crate::prelude::{is_host_server, SharedConfig};
use crate::shared::run_conditions::is_disconnected;
use crate::transport::io::IoDiagnosticsPlugin;

#[derive(Default, Debug)]
pub struct ClientDiagnosticsPlugin;

fn io_diagnostics_system(
    mut netclient: ResMut<ClientConnection>,
    time: Res<Time<Real>>,
    mut diagnostics: Diagnostics,
) {
    if let Some(io) = netclient.io_mut() {
        IoDiagnosticsPlugin::update_diagnostics(&mut io.stats, &time, &mut diagnostics);
    }
}
impl Plugin for ClientDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(IoDiagnosticsPlugin);
        app.add_systems(
            PostUpdate,
            io_diagnostics_system.run_if(not(is_host_server.or_else(is_disconnected))),
        );
    }
}
