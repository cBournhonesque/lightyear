# Reliability

Most game transports are built on top of something that can drop, duplicate, delay, or reorder packets.

That is not a bug. It is the normal shape of the internet.

The reliability layer is where Lightyear decides which data needs stronger guarantees and which data should be allowed to disappear.

## Why not make everything reliable?

Reliable ordered delivery is convenient, but it is not always what a game wants.

Imagine sending player aim direction 60 times per second. If one old aim packet is lost, waiting for it before applying newer aim packets is worse than dropping it. The newest value matters more than the complete history.

Now imagine sending "player joined team blue". That should arrive. It should probably arrive in order relative to other important setup messages.

Channels let you make that choice per category of data.

## The pieces

Lightyear's reliability layer has three main pieces:

- packet acknowledgements, handled by the packet header
- message acknowledgements, used by channels that care about delivery
- channel receivers, which enforce ordering or sequencing rules

Packets are acknowledged at the packet level. Reliable messages use that packet-level information to decide when a logical message or fragment has been delivered or should be retried.

## Acks and loss

Every outgoing packet gets a packet id. Every incoming packet tells the sender which packets it has seen recently. That lets both sides infer:

- this packet arrived
- this packet probably got lost
- this reliable message can stop being resent
- this reliable message or fragment needs another attempt

The loss timeout is based on the RTT estimate, with minimum and maximum bounds so packets are not marked lost instantly or kept forever.

## Channels

Channels are where you pick the behavior:

- unreliable for data that can be dropped
- reliable for data that must arrive
- ordered for data that must be processed in send order
- sequenced for data where only the newest value matters

The [channels](./channels.md) page covers the modes in more detail.
