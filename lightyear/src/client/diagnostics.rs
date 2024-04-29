use bevy::app::{App, Plugin, PostUpdate};
use bevy::diagnostic::Diagnostics;
use bevy::prelude::{Condition, IntoSystemConfigs, not, Real, Res, ResMut, Time};

use crate::client::networking::is_disconnected;
use crate::connection::client::{ClientConnection, NetClient};
use crate::prelude::SharedConfig;
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
            io_diagnostics_system.run_if(not(
                SharedConfig::is_host_server_condition.or_else(is_disconnected)
            )),
        );
    }
}
