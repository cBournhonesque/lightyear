//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
use bevy::app::{App, Plugin};
use bevy::diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic};
use bevy::prelude::{Real, Res, Resource, Time};
use crossbeam_channel::{Receiver, Sender};
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, SocketAddr};

#[cfg(feature = "metrics")]
use metrics;
use tracing::info;

use super::error::Result;
use super::{
    BoxedCloseFn, BoxedReceiver, BoxedSender, TransportBuilder, TransportBuilderEnum, LOCAL_SOCKET,
};
use crate::transport::local::{LocalChannel, LocalChannelBuilder};
use crate::transport::{PacketReceiver, PacketSender, Transport};

use crate::transport::middleware::conditioner::{
    ConditionedPacketReceiver, LinkConditioner, LinkConditionerConfig, PacketLinkConditioner,
};
use crate::transport::middleware::PacketReceiverWrapper;

// TODO: separate unconnected io from connected io? maybe similar 'states' generic as wtransport?
#[derive(Resource)]
pub struct Io {
    transport_builder: Option<TransportBuilderEnum>,
    local_addr: Option<SocketAddr>,
    // TODO: use enum dispatch on receiver/sender as well
    sender: Option<BoxedSender>,
    receiver: Option<BoxedReceiver>,
    close_fn: Option<BoxedCloseFn>,
    conditioner: Option<PacketLinkConditioner>,
    pub(crate) stats: IoStats,
}

impl Default for Io {
    fn default() -> Self {
        panic!("Io::default() is not implemented. Please provide an io");
    }
}

// TODO: add stats/compression to middleware
#[derive(Default, Debug)]
pub struct IoStats {
    pub bytes_sent: usize,
    pub bytes_received: usize,
    pub packets_sent: usize,
    pub packets_received: usize,
}

impl Io {
    pub(crate) fn new(
        transport_builder: TransportBuilderEnum,
        conditioner: Option<PacketLinkConditioner>,
    ) -> Self {
        Self {
            transport_builder: Some(transport_builder),
            local_addr: None,
            sender: None,
            receiver: None,
            close_fn: None,
            conditioner,
            stats: IoStats::default(),
        }
    }
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr.expect("The transport is not connected yet")
    }

    pub fn is_connected(&self) -> bool {
        self.sender.is_some()
    }

    pub fn connect(&mut self) -> Result<()> {
        // TODO: allow for connection retries
        let transport_builder = std::mem::take(&mut self.transport_builder)
            .expect("The transport has already been connected");
        let transport = transport_builder.connect()?;
        self.local_addr = Some(transport.local_addr());
        let (sender, receiver, close_fn) = transport.split();
        self.close_fn = close_fn;
        self.sender = Some(sender);
        if let Some(conditioner) = std::mem::take(&mut self.conditioner) {
            self.receiver = Some(Box::new(conditioner.wrap(receiver)));
        } else {
            self.receiver = Some(receiver);
        }
        Ok(())
    }

    // TODO: no stats are being computed here!
    pub fn split(&mut self) -> (&mut impl PacketSender, &mut impl PacketReceiver) {
        (
            self.sender
                .as_mut()
                .expect("The transport has not been connected"),
            self.receiver
                .as_mut()
                .expect("The transport has not been connected"),
        )
    }

    pub fn stats(&self) -> &IoStats {
        &self.stats
    }

    pub fn close(&mut self) -> Result<()> {
        if let Some(close_fn) = std::mem::take(&mut self.close_fn) {
            close_fn()?;
        }
        Ok(())
    }
}

impl Debug for Io {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Io").finish()
    }
}

impl PacketReceiver for Io {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        // todo: compression + bandwidth monitoring
        self.receiver.as_mut().unwrap().recv().map(|x| {
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

impl PacketSender for Io {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        // todo: compression + bandwidth monitoring
        #[cfg(feature = "metrics")]
        {
            metrics::counter!("transport.packets_sent").increment(1);
            metrics::gauge!("transport.bytes_sent").increment(payload.len() as f64);
        }
        self.stats.bytes_sent += payload.len();
        self.stats.packets_sent += 1;
        self.sender.as_mut().unwrap().send(payload, address)
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
        let delta_seconds = time.delta_seconds_f64();
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
