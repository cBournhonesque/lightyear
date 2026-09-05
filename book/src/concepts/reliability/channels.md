# Channels

Lightyear introduces the concept of a `Channel` to handle reliability.

A `Channel` is a way to send packets with specific reliability, ordering and priority guarantees.

You register a channel on the app like so (this must be shared between client and server, so it usually lives in the protocol plugin):
```rust,noplayground
pub struct Channel1;

pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);
    }
}
```

Any `Send + Sync + 'static` struct can be a channel; there is a blanket `Channel` impl, so no derive needed.

## Mode

The `mode` field of `ChannelSettings` defines the reliability/ordering guarantees of the channel.

Reliability:
- `Unreliable`: packets are not guaranteed to arrive (`UnorderedUnreliable`, `UnorderedUnreliableWithAcks`, `SequencedUnreliable`)
- `Reliable`: packets are guaranteed to arrive. We will resend the packet until we receive an acknowledgement from the remote.
  You can tune how often we resend via the `ReliableSettings` field (`rtt_resend_factor`, `rtt_resend_min_delay`).

Ordering:
- `Ordered`: packets are guaranteed to arrive in the order they were sent (*client sends 1,2,3,4,5, server receives 1,2,3,4,5*)
- `Unordered`: packets are not guaranteed to arrive in the order they were sent (*client sends 1,2,3,4,5, server receives 1,3,2,5,4*)
- `Sequenced`: packets are not guaranteed to arrive in the order they were sent, but we will discard packets that are older than the last received packet (*client sends 1,2,3,4,5, server receives 1,3,5 (2 and 4 are discarded)*)


## Direction

The direction (`NetworkDirection::ClientToServer`, `ServerToClient` or `Bidirectional`) can be used to restrict a `Channel` (or a message) to one way of traffic.
