use bevy::app::{App, Plugin, PostUpdate};
use bevy::diagnostic::Diagnostics;
use bevy::prelude::{Real, Res, ResMut, Time};

use crate::connection::client::{ClientConnection, NetClient};
use crate::prelude::Protocol;
use crate::transport::io::IoDiagnosticsPlugin;

pub struct ClientDiagnosticsPlugin<P> {
    _marker: std::marker::PhantomData<P>,
}

impl<P> Default for ClientDiagnosticsPlugin<P> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
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
impl<P: Protocol> Plugin for ClientDiagnosticsPlugin<P> {
    fn build(&self, app: &mut App) {
        app.add_plugins(IoDiagnosticsPlugin);
        app.add_systems(PostUpdate, io_diagnostics_system);
    }
}
