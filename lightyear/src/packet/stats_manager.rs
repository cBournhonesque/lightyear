/// Statistics for packets
pub(crate) mod packet {
    use core::time::Duration;
    use core::ops::{AddAssign, SubAssign};
    use tracing::trace;

    use crate::shared::time_manager::{TimeManager, WrappedTime};
    use crate::utils::ready_buffer::ReadyBuffer;
    type PacketStatsBuffer = ReadyBuffer<WrappedTime, PacketStats>;

    #[derive(Default, Copy, Clone, Debug, PartialEq)]
    struct PacketStats {
        num_sent_packets: u32,
        num_sent_packets_acked: u32,
        num_sent_packets_lost: u32,
        num_received_packets: u32,
    }

    impl AddAssign for PacketStats {
        fn add_assign(&mut self, other: Self) {
            self.num_sent_packets += other.num_sent_packets;
            self.num_sent_packets_acked += other.num_sent_packets_acked;
            self.num_sent_packets_lost += other.num_sent_packets_lost;
            self.num_received_packets += other.num_received_packets;
        }
    }

    impl SubAssign for PacketStats {
        fn sub_assign(&mut self, other: Self) {
            self.num_sent_packets -= other.num_sent_packets;
            self.num_sent_packets_acked -= other.num_sent_packets_acked;
            self.num_sent_packets_lost -= other.num_sent_packets_lost;
            self.num_received_packets -= other.num_received_packets;
        }
    }

    #[derive(Default, Debug)]
    struct FinalStats {
        packet_loss: f32,
    }

    #[derive(Debug)]
    pub(crate) struct PacketStatsManager {
        stats_buffer: PacketStatsBuffer,
        /// sum of the stats over the stats_buffer
        rolling_stats: PacketStats,
        /// stats accumulated for the current frame
        current_stats: PacketStats,
        /// Duration of the rolling buffer of stats to compute packet statistics
        stats_buffer_duration: Duration,
        final_stats: FinalStats,
    }

    impl Default for PacketStatsManager {
        fn default() -> Self {
            Self::new(Duration::from_secs(5))
        }
    }

    impl PacketStatsManager {
        pub(crate) fn new(stats_buffer_duration: Duration) -> Self {
            Self {
                stats_buffer: PacketStatsBuffer::new(),
                // sum of the stats over the stats_buffer
                rolling_stats: PacketStats::default(),
                // stats accumulated for the current frame
                current_stats: PacketStats::default(),
                stats_buffer_duration,
                final_stats: FinalStats::default(),
            }
        }

        pub(crate) fn update(&mut self, time_manager: &TimeManager) {
            // remove stats older than stats buffer duration
            let removed = self
                .stats_buffer
                .drain_until(&(time_manager.current_time() - self.stats_buffer_duration));
            for (_, stats) in removed {
                self.rolling_stats -= stats;
            }
            // add the current stats to the rolling stats
            let current_stats = core::mem::take(&mut self.current_stats);
            self.rolling_stats += current_stats;
            self.stats_buffer
                .push(time_manager.current_time(), current_stats);

            // compute stats
            self.compute_stats();
            trace!("stats buffer len: {}", self.stats_buffer.len());
            trace!("packet loss: {}", self.final_stats.packet_loss);
        }

        fn compute_stats(&mut self) {
            if self.rolling_stats.num_sent_packets > 0 {
                self.final_stats.packet_loss = self.rolling_stats.num_sent_packets_lost as f32
                    / self.rolling_stats.num_sent_packets as f32;
            }
        }

        // TODO: we could just emit raw stats, and then compute packet loss over an interval using prometheus/grafana
        /// Notify that a packet was sent
        pub(crate) fn sent_packet(&mut self) {
            #[cfg(feature = "metrics")]
            metrics::counter!("packets::sent").increment(1);

            self.current_stats.num_sent_packets += 1;
        }

        /// Notify that a packet we sent got lost (we did not receive an ack for it)
        pub(crate) fn sent_packet_lost(&mut self) {
            #[cfg(feature = "metrics")]
            metrics::counter!("packets::lost").increment(1);

            self.current_stats.num_sent_packets_lost += 1;
        }

        /// Notify that a packet we sent got acked
        pub(crate) fn sent_packet_acked(&mut self) {
            #[cfg(feature = "metrics")]
            metrics::counter!("packets::acked").increment(1);

            self.current_stats.num_sent_packets_acked += 1;
        }

        /// Notify that we received a packet
        pub(crate) fn received_packet(&mut self) {
            #[cfg(feature = "metrics")]
            metrics::counter!("packets::received").increment(1);

            self.current_stats.num_received_packets += 1;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_packet_stats() {
            let mut time_manager = TimeManager::default();
            let mut packet_stats_manager = PacketStatsManager::new(Duration::from_secs(2));
            // set the time to a value bigger than the stats buffer
            time_manager.update(Duration::from_secs(3));

            // add some packet data
            packet_stats_manager.sent_packet();
            packet_stats_manager.sent_packet();
            packet_stats_manager.sent_packet_lost();
            packet_stats_manager.sent_packet_acked();

            // update the packet stats
            packet_stats_manager.update(&time_manager);
            assert_eq!(packet_stats_manager.current_stats, PacketStats::default());
            assert_eq!(packet_stats_manager.stats_buffer.len(), 1);

            // compute final stats
            packet_stats_manager.compute_stats();
            assert_eq!(packet_stats_manager.final_stats.packet_loss, 1.0 / 2.0);

            // add some more packet data at a later time
            packet_stats_manager.sent_packet();
            packet_stats_manager.sent_packet_lost();
            time_manager.update(Duration::from_secs(1));
            assert_eq!(
                packet_stats_manager.current_stats,
                PacketStats {
                    num_sent_packets: 1,
                    num_sent_packets_acked: 0,
                    num_sent_packets_lost: 1,
                    num_received_packets: 0,
                }
            );
            packet_stats_manager.update(&time_manager);
            assert_eq!(packet_stats_manager.current_stats, PacketStats::default());
            assert_eq!(packet_stats_manager.stats_buffer.len(), 2);
            packet_stats_manager.compute_stats();
            assert_eq!(packet_stats_manager.final_stats.packet_loss, 2.0 / 3.0);

            // add some more packet data at a later time, the older stats should get removed
            packet_stats_manager.sent_packet();
            time_manager.update(Duration::from_secs(1));
            packet_stats_manager.update(&time_manager);
            assert_eq!(packet_stats_manager.current_stats, PacketStats::default());
            assert_eq!(packet_stats_manager.stats_buffer.len(), 2);
            assert_eq!(
                packet_stats_manager.rolling_stats,
                PacketStats {
                    num_sent_packets: 2,
                    num_sent_packets_acked: 0,
                    num_sent_packets_lost: 1,
                    num_received_packets: 0,
                }
            );
            packet_stats_manager.compute_stats();
            assert_eq!(packet_stats_manager.final_stats.packet_loss, 1.0 / 2.0);
        }
    }
}
