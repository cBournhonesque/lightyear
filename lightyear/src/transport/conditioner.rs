use bevy::utils::Duration;
/**
Contains the [`LinkConditionerConfig`] struct which can be used to simulate network conditions
*/
use std::io::Result;
use std::net::SocketAddr;

use cfg_if::cfg_if;
use rand;
use rand::{thread_rng, Rng};

use crate::transport::PacketReceiver;
use crate::utils::ready_buffer::ReadyBuffer;

cfg_if! {
    if #[cfg(any(test))] {
        use mock_instant::Instant;
    } else {
        use bevy::utils::Instant;
    }
}

/// Contains configuration required to initialize a LinkConditioner
#[derive(Clone, Debug)]
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

// Conditions a packet-receiver T that sends packets P
pub struct ConditionedPacketReceiver<T: PacketReceiver, P: Eq> {
    packet_receiver: T,
    config: LinkConditionerConfig,
    pub time_queue: ReadyBuffer<Instant, P>,
    last_packet: Option<P>,
}

impl<T: PacketReceiver, P: Eq> ConditionedPacketReceiver<T, P> {
    pub fn new(packet_receiver: T, link_conditioner_config: LinkConditionerConfig) -> Self {
        ConditionedPacketReceiver {
            packet_receiver,
            config: link_conditioner_config,
            time_queue: ReadyBuffer::new(),
            last_packet: None,
        }
    }
}

// Condition a packet by potentially adding latency/jitter/loss to it
fn condition_packet<P: Eq>(
    config: &LinkConditionerConfig,
    time_queue: &mut ReadyBuffer<Instant, P>,
    packet: P,
) {
    let mut rng = thread_rng();
    if rng.gen_range(0.0..1.0) <= config.incoming_loss {
        return;
    }
    let mut latency: i32 = config.incoming_latency.as_millis() as i32;
    // TODO: how can i use the virtual time here?
    let mut packet_timestamp = Instant::now();
    if config.incoming_jitter > Duration::default() {
        let jitter: i32 = config.incoming_jitter.as_millis() as i32;
        latency += rng.gen_range(-jitter..jitter);
    }
    if latency > 0 {
        packet_timestamp += Duration::from_millis(latency as u64);
    }
    time_queue.add_item(packet_timestamp, packet);
}

impl<T: PacketReceiver> PacketReceiver for ConditionedPacketReceiver<T, (SocketAddr, Box<[u8]>)> {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        loop {
            // keep trying to receive packets from the inner packet receiver
            match self.packet_receiver.recv() {
                Ok(option) => match option {
                    None => break,
                    // add conditioning (put the packets in the time queue)
                    Some((data, addr)) => condition_packet(
                        &self.config,
                        &mut self.time_queue,
                        (addr, data.to_vec().into_boxed_slice()),
                    ),
                },
                Err(err) => {
                    return Err(err);
                }
            }
        }
        // only return a packet if it is ready to be returned
        match self.time_queue.pop_item(&Instant::now()) {
            Some((_, (addr, data))) => {
                // we use `last_packet` to get ownership of the data
                self.last_packet = Some((addr, data));
                Ok(Some((self.last_packet.as_mut().unwrap().1.as_mut(), addr)))
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
