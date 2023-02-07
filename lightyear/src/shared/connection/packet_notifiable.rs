use crate::shared::PacketIndex;

/// Represents a manager that must be notified when packets have been dropped or
/// delivered
pub trait PacketNotifiable {
    /// Notifies the manager that a packet has been delivered
    fn notify_packet_delivered(&mut self, packet_index: PacketIndex);
}
