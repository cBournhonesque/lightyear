//! Contains the `LinkConditioner` struct which can be used to simulate network conditions
use bevy::reflect::Reflect;
#[cfg(feature = "std")]
use std::{io};
#[cfg(not(feature = "std"))]
use {
    alloc::{boxed::Box, format, string::String, vec, vec::Vec},
    no_std_io2::io,
};
use core::net::SocketAddr;

use cfg_if::cfg_if;
use core::time::Duration;
use rand;
use rand::{rng, Rng};

use crate::transport::error::Result;
use crate::transport::middleware::PacketReceiverWrapper;
use crate::transport::PacketReceiver;
use crate::utils::ready_buffer::ReadyBuffer;

cfg_if! {
    if #[cfg(test)] {
        use mock_instant::global::Instant;
    } else {
        use bevy::platform::time::Instant;
    }
}

/// Contains configuration required to initialize a LinkConditioner
#[derive(Clone, Debug, Reflect)]
pub struct LinkConditionerConfig {
    /// Delay to receive incoming messages in milliseconds (half the RTT)
    pub incoming_latency: Duration,
    /// The maximum additional random latency to delay received incoming
    /// messages in milliseconds. This may be added OR subtracted from the
    /// latency determined in the `incoming_latency` property above
    pub incoming_jitter: Duration,
    /// The % chance that an incoming packet will be dropped.
    /// Represented as a value between 0 and 1
    pub incoming_loss: f32,
}

pub(crate) type PacketLinkConditioner = LinkConditioner<(SocketAddr, Box<[u8]>)>;

pub(crate) struct LinkConditioner<P: Eq> {
    config: LinkConditionerConfig,
    pub time_queue: ReadyBuffer<Instant, P>,
    last_packet: Option<P>,
}

impl<P: Eq> LinkConditioner<P> {
    pub fn new(config: LinkConditionerConfig) -> Self {
        LinkConditioner {
            config,
            time_queue: ReadyBuffer::new(),
            last_packet: None,
        }
    }

    /// Add latency/jitter/loss to a packet
    fn condition_packet(&mut self, packet: P) {
        let mut rng = rng();
        if rng.random_range(0.0..1.0) <= self.config.incoming_loss {
            return;
        }
        let mut latency: i32 = self.config.incoming_latency.as_millis() as i32;
        // TODO: how can i use the virtual time here?
        let mut packet_timestamp = Instant::now();
        if self.config.incoming_jitter > Duration::default() {
            let jitter: i32 = self.config.incoming_jitter.as_millis() as i32;
            latency += rng.random_range(-jitter..jitter);
        }
        if latency > 0 {
            packet_timestamp += Duration::from_millis(latency as u64);
        }
        self.time_queue.push(packet_timestamp, packet);
    }

    /// Check if a packet is ready to be returned
    fn pop_packet(&mut self) -> Option<P> {
        self.time_queue
            .pop_item(&Instant::now())
            .map(|(_, packet)| packet)
    }
}

impl<T: PacketReceiver> PacketReceiverWrapper<T> for LinkConditioner<(SocketAddr, Box<[u8]>)> {
    fn wrap(self, receiver: T) -> impl PacketReceiver {
        ConditionedPacketReceiver {
            packet_receiver: receiver,
            conditioner: self,
        }
    }
}

/// A wrapper around a packet receiver that simulates network conditions
/// by adding latency, jitter and packet loss to incoming packets.
pub struct ConditionedPacketReceiver<T: PacketReceiver, P: Eq> {
    packet_receiver: T,
    conditioner: LinkConditioner<P>,
}

impl<T: PacketReceiver> PacketReceiver for ConditionedPacketReceiver<T, (SocketAddr, Box<[u8]>)> {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        loop {
            // keep trying to receive packets from the inner packet receiver
            let option = self.packet_receiver.recv()?;
            match option {
                None => break,
                // add conditioning (put the packets in the time queue)
                Some((data, addr)) => self
                    .conditioner
                    .condition_packet((addr, data.to_vec().into_boxed_slice())),
            }
        }
        // only return a packet if it is ready to be returned
        match self.conditioner.pop_packet() {
            Some((addr, data)) => {
                // we use `last_packet` to get ownership of the data
                self.conditioner.last_packet = Some((addr, data));
                Ok(Some((
                    self.conditioner.last_packet.as_mut().unwrap().1.as_mut(),
                    addr,
                )))
            }
            None => Ok(None),
        }
    }
}

impl LinkConditionerConfig {
    /// Creates a new LinkConditionerConfig
    pub fn new(incoming_latency: Duration, incoming_jitter: Duration, incoming_loss: f32) -> Self {
        LinkConditionerConfig {
            incoming_latency,
            incoming_jitter,
            incoming_loss,
        }
    }

    /// Creates a new LinkConditioner that simulates a connection which is in a
    /// good condition
    pub fn good_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(40),
            incoming_jitter: Duration::from_millis(6),
            incoming_loss: 0.002,
        }
    }

    /// Creates a new `LinkConditioner` that simulates a connection which is in an
    /// average condition
    pub fn average_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(170),
            incoming_jitter: Duration::from_millis(45),
            incoming_loss: 0.02,
        }
    }

    /// Creates a new `LinkConditioner` that simulates a connection which is in an
    /// poor condition
    pub fn poor_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(300),
            incoming_jitter: Duration::from_millis(84),
            incoming_loss: 0.04,
        }
    }
}
