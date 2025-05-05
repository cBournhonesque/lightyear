//! Compute Diagnostics based on ping statistics (jitter, RTT)

use crate::shared::ping::manager::PingManager;
use bevy::app::{App, Plugin};
use bevy::diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic};
use core::time::Duration;

/// Plugin to compute some network diagnostics related to pings
pub struct PingDiagnosticsPlugin {
    pub history_len: usize,
    pub flush_interval: Duration,
}

impl Default for PingDiagnosticsPlugin {
    fn default() -> Self {
        Self {
            history_len: 60,
            flush_interval: Duration::from_millis(100),
        }
    }
}

impl PingDiagnosticsPlugin {
    /// Jitter
    pub const JITTER: DiagnosticPath = DiagnosticPath::const_new("ping.jitter.ms");

    /// Round Trip Time (RTT)
    pub const RTT: DiagnosticPath = DiagnosticPath::const_new("ping.rtt.ms");

    /// Number of pings sent
    pub const PINGS_SENT: DiagnosticPath = DiagnosticPath::const_new("ping.ping_sent_count");

    /// Number of pongs received
    pub const PONGS_RECEIVED: DiagnosticPath =
        DiagnosticPath::const_new("ping.pong_received_count");

    pub(crate) fn add_measurements(manager: &PingManager, mut diagnostics: Diagnostics) {
        diagnostics.add_measurement(&Self::JITTER, || manager.jitter().as_secs_f64() * 1000.0);
        diagnostics.add_measurement(&Self::RTT, || manager.rtt().as_secs_f64() * 1000.0);
        diagnostics.add_measurement(&Self::PINGS_SENT, || manager.pings_sent as f64);
        diagnostics.add_measurement(&Self::PONGS_RECEIVED, || manager.pongs_recv as f64);
    }
}

impl Plugin for PingDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.register_diagnostic(
            Diagnostic::new(Self::JITTER)
                .with_suffix("ms")
                .with_max_history_length(self.history_len),
        );
        app.register_diagnostic(
            Diagnostic::new(Self::RTT)
                .with_suffix("ms")
                .with_max_history_length(self.history_len),
        );
        app.register_diagnostic(
            Diagnostic::new(Self::PINGS_SENT)
                .with_suffix("")
                .with_max_history_length(self.history_len),
        );
        app.register_diagnostic(
            Diagnostic::new(Self::PONGS_RECEIVED)
                .with_suffix("")
                .with_max_history_length(self.history_len),
        );
    }
}
