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