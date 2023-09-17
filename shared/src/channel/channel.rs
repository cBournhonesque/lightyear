#[derive(Clone)]
pub struct ChannelSettings {
    pub mode: ChannelMode,
    pub direction: ChannelDirection,
}

pub enum ChannelOrdering {
    /// Messages will arrive in the order that they were sent
    Ordered,
    /// Messages will arrive in any order
    Unordered,
    /// Only the newest messages are accepted; older messages are discarded
    Sequenced,
}

#[derive(Clone)]
/// ChannelMode specifies how packets are sent and received
/// See more information: http://www.jenkinssoftware.com/raknet/manual/reliabilitytypes.html
pub enum ChannelMode {
    /// Packets may arrive out-of-order, or not at all
    UnorderedUnreliable,
    /// Same as unordered unreliable, but only the newest packet is ever accepted, older packets
    /// are ignored
    SequencedUnreliable,
    /// Packets may arrive out-of-order, but we make sure (with retries, acks) that the packet
    /// will arrive
    UnorderedReliable(ReliableSettings),
    /// Same as unordered reliable, but the packets are sequenced (only the newest packet is accepted)
    SequencedReliable(ReliableSettings),
    /// Packets will arrive in the correct order at the destination
    OrderedReliable(ReliableSettings),
}

#[derive(Clone, Eq, PartialEq)]
pub enum ChannelDirection {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}

#[derive(Clone)]
pub struct ReliableSettings {
    /// TODO
    pub rtt_resend_factor: f32,
}

impl ReliableSettings {
    pub const fn default() -> Self {
        Self {
            rtt_resend_factor: 1.5,
        }
    }
}
