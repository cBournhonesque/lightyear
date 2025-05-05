//! Collect diagnostics for the prediction systems.

use crate::prelude::{client::is_disconnected, is_host_server};
use bevy::diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;

/// Plugin in charge of collecting diagnostics for the prediction systems.
pub struct PredictionDiagnosticsPlugin {
    /// Number of diagnostics to keep in history
    history_length: usize,
    /// How often to flush the stored data into the Diagnostics
    flush_interval: Duration,
}

impl Default for PredictionDiagnosticsPlugin {
    fn default() -> Self {
        Self {
            history_length: 60,
            flush_interval: Duration::from_millis(200),
        }
    }
}

impl PredictionDiagnosticsPlugin {
    /// Number of rollbacks
    pub const ROLLBACKS: DiagnosticPath =
        DiagnosticPath::const_new("replication.prediction.rollbacks");

    /// Total number of ticks resimulated as part of rollbacks
    pub const ROLLBACK_TICKS: DiagnosticPath =
        DiagnosticPath::const_new("replication.prediction.rollback_ticks");

    /// Average rollback depth
    pub const ROLLBACK_DEPTH: DiagnosticPath =
        DiagnosticPath::const_new("replication.prediction.rollback_depth");

    fn flush_measurements(metrics: ResMut<PredictionMetrics>, mut diagnostics: Diagnostics) {
        diagnostics.add_measurement(&Self::ROLLBACKS, || metrics.rollbacks as f64);
        diagnostics.add_measurement(&Self::ROLLBACK_TICKS, || metrics.rollback_ticks as f64);
        diagnostics.add_measurement(&Self::ROLLBACK_DEPTH, || {
            if metrics.rollbacks == 0 {
                0.0
            } else {
                metrics.rollback_ticks as f64 / metrics.rollbacks as f64
            }
        });
    }
}

/// Client metrics resource. Flushed to Diagnostics system periodically.
#[derive(Default, Resource, Debug, Reflect)]
#[reflect(Resource)]
pub struct PredictionMetrics {
    /// Incremented once per rollback
    pub rollbacks: u32,
    /// Per rollback, incremented by the number of ticks the rollback window contains
    pub rollback_ticks: u32,
}

impl Plugin for PredictionDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        let should_run = on_timer(self.flush_interval).and(not(is_host_server.or(is_disconnected)));

        app.register_type::<PredictionMetrics>();

        app.init_resource::<PredictionMetrics>();
        app.add_systems(PostUpdate, Self::flush_measurements.run_if(should_run));
        app.register_diagnostic(
            Diagnostic::new(Self::ROLLBACKS)
                .with_suffix("rollbacks")
                .with_max_history_length(self.history_length),
        );
        app.register_diagnostic(
            Diagnostic::new(Self::ROLLBACK_TICKS)
                .with_suffix("ticks resimulated during rollback")
                .with_max_history_length(self.history_length),
        );
        app.register_diagnostic(
            Diagnostic::new(Self::ROLLBACK_DEPTH)
                .with_suffix("Average rollback depth")
                .with_max_history_length(self.history_length),
        );
    }
}
