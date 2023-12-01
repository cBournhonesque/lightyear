# Channels


Lightyear introduces the concept of a `Channel` to handle reliability.

A `Channel` is a way to send packets with specific reliability, ordering and priority guarantees.

You can add a channel to your protocol like so:
```rust,noplayground
#[derive(Channel)]
struct MyChannel;

pub fn protocol() -> MyProtocol {
    let mut p = MyProtocol::default();
    p.add_channel::<MyChannel>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        direction: ChannelDirection::Bidirectional,
    });
    p
}
``` 

## Mode

The `mode` field of `ChannelSettings` defines the reliability/ordering guarantees of the channel.

Reliability:
- `Unreliable`: packets are not guaranteed to arrive
- `Reliable`: packets are guaranteed to arrive. We will resend the packet until we receive an acknowledgement from the remote.
  You can define how often we resend the packet via the `ReliableSettings` field.

Ordering:
- `Ordered`: packets are guaranteed to arrive in the order they were sent (*client sends 1,2,3,4,5, server receives 1,2,3,4,5*)
- `Unordered`: packets are not guaranteed to arrive in the order they were sent (*client sends 1,2,3,4,5, server receives 1,3,2,5,4*)
- `Sequenced`: packets are not guaranteed to arrive in the order they were sent, but we will discard packets that are older than the last received packet (*client sends 1,2,3,4,5, server receives 1,3,5 (2 and 4 are discarded)*)


## Direction

The `direction` field can be used to restrict a `Channel` from sending packets from client->server or server->client.

## Priority (TODO)

In case the total bandwidth is limited, you can use the `priority` field to define which channels should be prioritized over others.
Lightyear uses a priority with accumulation scheme:
- Each channel has a priority accumulation value
- Every frame, we try to send as many packets as possible, starting with the highest priority channel (ties are broken randomly)
- If we can't send messages through a channel (because the bandwidth is full), we accumulate the priority value of the channel.
- If we can send messages through a channel, we reset the priority accumulation value to the starting value.

For example, we have channel A with priority 10, and channel B with priority 1; and we only have enough bandwidth to send 1 packet per frame.
- Frame 1: channel A has priority 10, channel B has priority 1. We send a packet through channel A, and reset its priority to 10. We update the priority of channel B to 2.
- Frame 2: channel A has priority 10, channel B has priority 2. We send a packet through channel A, and reset its priority to 10. We update the priority of channel B to 3.
- ...
- Frame 11: channel A has priority 10, channel B has priority 11. We send a packet through channel B, and reset its priority to 1. We update the priority of channel A to 20.


To limit the bandwidth, compute the amount of bytes sent in the last X seconds?