# Channels

A channel is a typed lane through the transport.

Each channel has its own reliability, ordering, send frequency, and priority. You can use different channels for different kinds of traffic instead of forcing every message to behave the same way.

## Registering a channel

Channels are marker types:

```rust,ignore
pub struct ChatChannel;
```

Register them in the shared protocol:

```rust,ignore
app.add_channel::<ChatChannel>(ChannelSettings {
    mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
    send_frequency: Duration::default(),
    priority: 1.0,
    ..default()
})
.add_direction(NetworkDirection::Bidirectional);
```

`add_direction` controls which side gets senders and receivers for that channel.

## Channel settings

`ChannelSettings` has three important fields:

- `mode`: reliability and ordering behavior
- `send_frequency`: how often the channel should try to flush buffered messages
- `priority`: how important this channel is when bandwidth is limited

The final priority of a message is the message priority multiplied by the channel priority.

## Modes

### `UnorderedUnreliable`

Messages may arrive out of order or not at all.

Use this for high-rate data where old values are not important: aim direction, cosmetic state, debug draw data, or anything that will be replaced soon.

### `UnorderedUnreliableWithAcks`

Messages may still be dropped, but Lightyear tracks acknowledgements for them.

This is useful when you do not want retries, but you still want to know whether something arrived.

### `SequencedUnreliable`

Messages may be dropped, and older messages are discarded when newer ones have already arrived.

Use this for "latest value wins" state. It is often a better fit than ordered delivery for real-time movement-adjacent data.

### `UnorderedReliable`

Every message is retried until acknowledged, but messages do not have to be delivered to the receiver in send order.

Use this when every item matters but order does not: asset chunk availability, independent notifications, or a set of state corrections that can be applied individually.

### `SequencedReliable`

Messages are reliable, but older messages can be ignored once a newer message in the sequence is accepted.

This is less common, but useful when you need the latest state to arrive eventually and do not care about obsolete intermediate states.

### `OrderedReliable`

Messages arrive reliably and in order.

Use this for state machines and commands where order is part of correctness: connection setup, inventory transactions, chat messages, match state transitions, or anything where processing message 5 before message 4 would be wrong.

Do not use ordered reliable as the default for high-rate gameplay state. One lost message can hold back everything behind it.

## Reliable settings

Reliable modes take `ReliableSettings`:

```rust,ignore
ReliableSettings {
    rtt_resend_factor: 1.5,
    rtt_resend_min_delay: Duration::from_millis(20),
}
```

The resend delay is based on the current RTT estimate. A low delay repairs loss faster, but can send unnecessary duplicates on jittery links. A high delay saves bandwidth, but makes recovery slower.

## Choosing channels

A simple starting point is:

- ordered reliable for setup, chat, and important commands
- sequenced unreliable for frequent latest-value messages
- reliable unordered for independent events that must arrive
- leave replication and inputs on their own Lightyear-managed paths unless you have a specific reason to customize them

If you are unsure, write down what should happen when a packet is lost. The right channel mode usually follows from that answer.
