//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use std::fmt::{Debug, Formatter};
use std::net::SocketAddr;

use bevy::diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic};
use bevy::prelude::*;
#[cfg(feature = "metrics")]
use metrics;

use crate::transport::{PacketReceiver, PacketSender};

use super::error::Result;
use super::{BoxedReceiver, BoxedSender};

/// Connected io layer that can send/receive bytes
#[derive(Resource)]
pub struct BaseIo<T: Send + Sync> {
    pub(crate) local_addr: SocketAddr,
    pub(crate) sender: BoxedSender,
    pub(crate) receiver: BoxedReceiver,
    pub(crate) state: IoState,
    pub(crate) stats: IoStats,
    pub(crate) context: T,
}

// TODO: add stats/compression to middleware
#[derive(Default, Debug, Reflect)]
pub struct IoStats {
    pub bytes_sent: usize,
    pub bytes_received: usize,
    pub packets_sent: usize,
    pub packets_received: usize,
}

impl<T: Send + Sync> BaseIo<T> {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    // TODO: no stats are being computed here!
    pub fn split(&mut self) -> (&mut impl PacketSender, &mut impl PacketReceiver) {
        (&mut self.sender, &mut self.receiver)
    }

    pub fn stats(&self) -> &IoStats {
        &self.stats
    }
}

impl<T: Send + Sync> Debug for BaseIo<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Io").finish()
    }
}

impl<T: Send + Sync> PacketReceiver for BaseIo<T> {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        // todo: bandwidth monitoring
        self.receiver.as_mut().recv().map(|x| {
            if let Some((ref buffer, _)) = x {
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("transport.packets_received").increment(1);
                    metrics::gauge!("transport.bytes_received").increment(buffer.len() as f64);
                }
                self.stats.bytes_received += buffer.len();
                self.stats.packets_received += 1;
            }
            x
        })
    }
}

impl<T: Send + Sync> PacketSender for BaseIo<T> {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        // todo: bandwidth monitoring
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("transport.packets_sent").increment(1);
            metrics::gauge!("transport.bytes_sent").increment(payload.len() as f64);
        }
        self.stats.bytes_sent += payload.len();
        self.stats.packets_sent += 1;
        self.sender.as_mut().send(payload, address)
    }
}

pub struct IoDiagnosticsPlugin;

impl IoDiagnosticsPlugin {
    /// How many bytes do we receive per second
    pub const BYTES_IN: DiagnosticPath = DiagnosticPath::const_new("KB received per second");
    /// How many bytes do we send per second
    pub const BYTES_OUT: DiagnosticPath = DiagnosticPath::const_new("KB sent per second");

    /// How many bytes do we receive per second
    pub const PACKETS_IN: DiagnosticPath = DiagnosticPath::const_new("packets received per second");
    /// How many bytes do we send per second
    pub const PACKETS_OUT: DiagnosticPath = DiagnosticPath::const_new("packets sent per second");

    /// Max diagnostic history length.
    pub const DIAGNOSTIC_HISTORY_LEN: usize = 60;

    pub(crate) fn update_diagnostics(
        stats: &mut IoStats,
        time: &Res<Time<Real>>,
        diagnostics: &mut Diagnostics,
    ) {
        let delta_seconds = time.delta_secs_f64();
        if delta_seconds == 0.0 {
            return;
        }
        diagnostics.add_measurement(&Self::BYTES_IN, || {
            (stats.bytes_received as f64 / 1000.0) / delta_seconds
        });
        diagnostics.add_measurement(&Self::BYTES_OUT, || {
            (stats.bytes_sent as f64 / 1000.0) / delta_seconds
        });
        diagnostics.add_measurement(&Self::PACKETS_IN, || {
            stats.packets_received as f64 / delta_seconds
        });
        diagnostics.add_measurement(&Self::PACKETS_OUT, || {
            stats.packets_sent as f64 / delta_seconds
        });
        *stats = IoStats::default()
    }
}

impl Plugin for IoDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        app.register_diagnostic(
            Diagnostic::new(IoDiagnosticsPlugin::BYTES_IN)
                .with_max_history_length(IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN),
        );
        app.register_diagnostic(
            Diagnostic::new(IoDiagnosticsPlugin::BYTES_OUT)
                .with_max_history_length(IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN),
        );
        app.register_diagnostic(
            Diagnostic::new(IoDiagnosticsPlugin::PACKETS_IN)
                .with_max_history_length(IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN),
        );
        app.register_diagnostic(
            Diagnostic::new(IoDiagnosticsPlugin::PACKETS_OUT)
                .with_max_history_length(IoDiagnosticsPlugin::DIAGNOSTIC_HISTORY_LEN),
        );
    }
}

/// Tracks the state of the Io
#[derive(Debug, PartialEq, Reflect)]
pub(crate) enum IoState {
    Connecting,
    Connected,
    Disconnected,
}
