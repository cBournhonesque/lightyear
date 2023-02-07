use std::{collections::HashMap, net::SocketAddr, time::Duration};

use crate::shared::BandwidthMonitor as SingleBandwidthMonitor;

pub struct BandwidthMonitor {
    total_monitor: SingleBandwidthMonitor,
    client_monitors: HashMap<SocketAddr, SingleBandwidthMonitor>,
    bandwidth_measure_duration: Duration,
}

impl BandwidthMonitor {
    pub fn new(bandwidth_measure_duration: Duration) -> Self {
        BandwidthMonitor {
            bandwidth_measure_duration,
            total_monitor: SingleBandwidthMonitor::new(bandwidth_measure_duration),
            client_monitors: HashMap::new(),
        }
    }

    pub fn create_client(&mut self, address: &SocketAddr) {
        self.client_monitors.insert(
            *address,
            SingleBandwidthMonitor::new(self.bandwidth_measure_duration),
        );
    }

    pub fn delete_client(&mut self, address: &SocketAddr) {
        self.client_monitors.remove(address);
    }

    pub fn record_packet(&mut self, address: &SocketAddr, bytes: usize) {
        if let Some(client_monitor) = self.client_monitors.get_mut(address) {
            client_monitor.record_packet(bytes);

            self.total_monitor.record_packet(bytes);
        }
    }

    pub fn total_bandwidth(&mut self) -> f32 {
        self.total_monitor.bandwidth()
    }

    pub fn client_bandwidth(&mut self, address: &SocketAddr) -> f32 {
        self.client_monitors
            .get_mut(address)
            .expect("client associated with address does not exist")
            .bandwidth()
    }
}
