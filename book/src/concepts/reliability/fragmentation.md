# Fragmentation

Fragmentation is what lets Lightyear send a logical message that is too large to fit in one packet.

The packet MTU is kept small, currently about 1200 bytes, so packets can travel over UDP without relying on IP fragmentation. IP fragmentation is painful for games: if one IP fragment is lost, the whole packet is lost, and many networks handle fragmented UDP poorly.

Lightyear fragments at the message layer instead.

## Message fragments

A message is serialized before it is packed into packets. If the serialized bytes are larger than the fragment limit, the channel sender splits it into fragments.

Each fragment carries:

- the message id
- the fragment index
- the total number of fragments
- the compression marker on the first fragment
- the bytes for that slice

The receiver stores fragments by message id. When all fragments arrive, it reassembles the original bytes and hands the message to the channel receiver.

## Reliable and unreliable channels

Fragmentation works best with reliable channels. A reliable sender can keep track of which fragments were acknowledged and resend the missing ones.

Fragmented unreliable messages can be sent, but they are fragile. If any fragment is lost, the receiver cannot reconstruct the message. That is usually fine for data you are willing to drop, but it is the wrong choice for large important payloads.

The practical advice is:

- keep high-rate unreliable messages small
- use reliable channels for large payloads that must arrive
- avoid sending very large gameplay messages every tick

## Cleanup

The receiver cannot keep partial messages forever. If fragments for a message stop arriving, the partial reassembly state is eventually cleaned up.

That is another reason to keep large messages rare. Fragmentation is useful, but it is not a replacement for designing compact messages.

## Compression

If compression is enabled, Lightyear can compress a large message before fragmenting it. The first fragment records which compression mode was used, so the receiver knows how to decompress the reassembled payload.

Compression helps most when the payload has repeated structure. It can also waste CPU on small or already-compressed data. Measure before assuming it is a win.
