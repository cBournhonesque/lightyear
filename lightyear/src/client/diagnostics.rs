use bevy::app::{App, Plugin, PostUpdate, Update};
use bevy::diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic};
use bevy::ecs::system::Resource;
use bevy::prelude::{not, Condition, IntoSystemConfigs, Real, Res, ResMut, Time};
use bevy::time::common_conditions::on_timer;
use instant::Duration;

use crate::connection::client::{ClientConnection, NetClient};
use crate::prelude::{is_host_server, SharedConfig};
use crate::shared::run_conditions::is_disconnected;
use crate::transport::io::IoDiagnosticsPlugin;

#[derive(Default, Debug)]
pub struct ClientDiagnosticsPlugin;

/// Client metrics resource. Flushed to Diagnostics system periodically.
#[derive(Default, Resource, Debug)]
pub struct ClientMetrics {
    /// Incremented once per rollback
    pub rollbacks: u32,
    /// Per rollback, incremented by the number of ticks the rollback window contains
    pub rollback_ticks: u32,
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

impl Plugin for ClientDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(IoDiagnosticsPlugin);
        app.add_systems(
            PostUpdate,
            io_diagnostics_system.run_if(not(is_host_server.or_else(is_disconnected))),
        );
        app.init_resource::<ClientMetrics>();
        app.add_systems(
            Update,
            Self::add_measurements.run_if(on_timer(Duration::from_millis(100))),
        );
        app.register_diagnostic(
            Diagnostic::new(Self::ROLLBACKS)
                .with_suffix("rollbacks")
                .with_max_history_length(Self::DIAGNOSTIC_HISTORY_LEN),
        );
        app.register_diagnostic(
            Diagnostic::new(Self::ROLLBACK_TICKS)
                .with_suffix("ticks resimulated during rollback")
                .with_max_history_length(Self::DIAGNOSTIC_HISTORY_LEN),
        );
        app.register_diagnostic(
            Diagnostic::new(Self::ROLLBACK_DEPTH)
                .with_suffix("Average rollback depth")
                .with_max_history_length(Self::DIAGNOSTIC_HISTORY_LEN),
        );
    }
}

impl ClientDiagnosticsPlugin {
    /// Max diagnostic history length.
    pub const DIAGNOSTIC_HISTORY_LEN: usize = 60;
    /// Number of rollbacks
    pub const ROLLBACKS: DiagnosticPath =
        DiagnosticPath::const_new("replication.prediction.rollbacks");
    /// Total number of ticks resimulated as part of rollbacks
    pub const ROLLBACK_TICKS: DiagnosticPath =
        DiagnosticPath::const_new("replication.prediction.rollback_ticks");
    /// Average rollback depth
    pub const ROLLBACK_DEPTH: DiagnosticPath =
        DiagnosticPath::const_new("replication.prediction.rollback_depth");

    fn add_measurements(metrics: ResMut<ClientMetrics>, mut diagnostics: Diagnostics) {
        diagnostics.add_measurement(&Self::ROLLBACKS, || metrics.rollbacks as f64);
        diagnostics.add_measurement(&Self::ROLLBACK_TICKS, || metrics.rollback_ticks as f64);
        diagnostics.add_measurement(&Self::ROLLBACK_DEPTH, || {
            if metrics.rollbacks == 0 {
                0.0
            } else {
                metrics.rollback_ticks as f64 / metrics.rollbacks as f64
            }
        });
        // don't wipe metrics, store totals.
        // *metrics = ClientMetrics::default();
    }
}
