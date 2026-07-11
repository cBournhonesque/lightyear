# Allocation Regression Notes

Status date: 2026-06-26

This is a living note for making Lightyear's packet and higher-level hot paths allocation-stable.
Keep it updated when allocation budgets change, buffers are pooled, or the measured scope changes.

## Scope

The first regression target is the transport packet send/receive loop only:

- packet input is already prepared as `Bytes`;
- no Bevy schedule execution;
- no typed `lightyear_messages` serialization;
- no IO backend;
- no netcode, replication, or compression;
- one channel batch per packet-loop iteration, four 64-byte unfragmented messages per batch.

The test warms up with 1,500 messages, then measures 1,000 messages. With four messages per batch
this currently warms up 375 packet-loop iterations and measures 250 packet-loop iterations. The
longer warmup lets naturally-grown cached allocations, including the packet stats rolling window,
settle before measurement. Batch preparation happens before the measured region so the allocator
count isolates packet build + parse rather than fixture setup. The fixture also processes packet
headers on receive and clears the ACK/loss buffers that the transport system would normally drain.

The fixture lives behind `lightyear_transport/test_utils` so it can use crate-private packet
builder/parser APIs without making them production API. It parses packet payloads from borrowed
slices and returns packet payload buffers to `PacketBuilder` after receive-side parsing.

## Current Tests

Run the allocation budget test:

```sh
cargo test -p lightyear_transport --features test_utils --test allocation_regression -- --nocapture
```

Run the DHAT heap profile:

```sh
cargo test -p lightyear_transport --features test_utils --test dhat_packet_loop -- --ignored --nocapture
```

The DHAT run writes:

```text
target/dhat-packet-loop.json
```

Open that file with DHAT's viewer when call-site attribution is needed.

## Current Implementation

- `PacketBuilder` owns a `BufferPool` so packet-loop payloads and metadata vectors can reuse heap
  allocations after warmup instead of allocating new buffers for each packet.
- `BufferPool` owns the MTU-sized packet payload pool, the returned `Vec<Packet>` pool, and the
  message metadata vector pool used by `metrics` builds.
- `PacketBuilder::finalize_packet` no longer calls `shrink_to_fit`.
- `PacketStatsManager` grows its rolling stats buffer naturally and drains old samples without
  allocating a temporary collection.
- The production send loop recycles the returned packet list allocation and any packet payloads that
  are rejected by bandwidth quota. It also recycles drained message metadata vectors after ack and
  metrics bookkeeping.

Production caveat: accepted packets are still converted into `Bytes` and pushed into `Link`. Once
that happens, the `Vec<u8>` payload is owned by `Bytes` and cannot be returned to `PacketBuilder`.
The regression fixture proves the packet builder/parser loop can run without allocations when the
payload is returned, but the full production IO path still needs a reusable send-payload ownership
model, or direct IO writes before recycling, to reuse buffers for accepted sent packets.

## Current Budget

Current guardrail:

- allocations: `0`
- reallocations: `0`
- allocated bytes: `0`

Observed on 2026-06-26:

- `stats_alloc`: 0 allocations, 0 reallocations, 0 allocated bytes.
- `dhat-rs`: 0 blocks, 0 total bytes; peak live heap 0 bytes in 0 blocks.

Previous DHAT attribution before packet-buffer recycling:

- `PacketBuilder::get_new_buffer` / `build_new_single_packet`: 500 blocks, 371,750 bytes. This is
  the per-packet payload buffer path, including the resize/shrink behavior associated with the
  packet payload allocation.
- `PacketBuilder::build_packets_internal`: 250 blocks, 88,000 bytes from `Vec<Packet>` output
  growth.
- `Reader::split_len` through `Bytes::slice`: 250 blocks, 6,000 bytes. Slicing a Vec-backed packet
  payload allocates shared `Bytes` backing metadata.
- `PacketStatsManager::update` / `ReadyBuffer::push`: 4 blocks, 30,720 bytes. This was bounded
  stats bookkeeping growth rather than per-packet payload churn.

Keep this exact-zero budget for the packet-builder fixture. Add separate budget tests for broader
paths instead of weakening this one.

## Beyond Transport Scope

The second regression target covers higher-level loops that can be measured without pulling in
unrelated Bevy schedule or IO allocations:

- `lightyear_messages` queue loop: typed `MessageSender<M>` buffering, draining, typed
  `MessageReceiver<M>` buffering, and receiver draining. The fixture sends eight messages per
  batch.
- `lightyear_messages` serialization loop: fixed-size `ToBytes` message serialization through
  `MessageRegistry`, reusable `Writer::split`, `Reader`, and typed deserialization.
- `lightyear_prediction` history loop: `PredictionHistory<C>` appending predicted component states
  and pruning to a 64-tick rolling window with `clear_until_tick`.
- `lightyear_interpolation` history loop: `ConfirmedHistory<C>` appending monotonic explicit
  samples or unchanged anchors, keeping the interpolation bracketing pair with `pop_present`, and
  applying a registered interpolation function.

These tests warm up with 100 messages/ticks and measure the following 1,000. The warmup lets
message queues, `Writer`, and `VecDeque` history buffers reach their steady capacities before
measurement starts.

These tests intentionally do not claim that every component/message type is allocation-free.
`Vec`, `String`, `Bytes` payload slicing, serde formats, or custom interpolation/prediction code can
allocate. Component clones in prediction, interpolation, and frame interpolation are only
heap-allocation-free when the component's `Clone` is heap-allocation-free.

Production prediction now prunes component histories after
`PredictionManager.rollback_policy.max_rollback_ticks + PREDICTION_HISTORY_TICK_MARGIN`. The prune
applies to `PredictionHistory<C>` and compacts prediction-side `ConfirmedHistory<C>` to an anchor at
the cutoff tick, so a component whose authoritative value last changed long before the rollback
window still has a last-confirmed value for unchanged-state comparisons.

Run the higher-level allocation budget tests:

```sh
cargo test -p lightyear_messages --features test_utils --test allocation_regression -- --nocapture
cargo test -p lightyear_prediction --test allocation_regression -- --nocapture
cargo test -p lightyear_interpolation --test allocation_regression -- --nocapture
```

Run the higher-level DHAT heap profiles:

```sh
cargo test -p lightyear_messages --features test_utils --test dhat_message_loop -- --ignored --nocapture
cargo test -p lightyear_prediction --test dhat_prediction_history_loop -- --ignored --nocapture
cargo test -p lightyear_interpolation --test dhat_interpolation_history_loop -- --ignored --nocapture
```

The DHAT runs write:

```text
target/dhat-message-loop.json
target/dhat-prediction-history-loop.json
target/dhat-interpolation-history-loop.json
```

Higher-level observed results on 2026-06-26:

- message queue loop: `stats_alloc` 0 allocations, 0 reallocations, 0 allocated bytes; `dhat-rs`
  0 blocks, 0 total bytes.
- message serialization loop: `stats_alloc` 0 allocations, 0 reallocations, 0 allocated bytes;
  included in the message DHAT run, which reported 0 blocks and 0 total bytes for the combined
  message measured region.
- prediction history loop: `stats_alloc` 0 allocations, 0 reallocations, 0 allocated bytes;
  `dhat-rs` 0 blocks, 0 total bytes.
- interpolation history loop: `stats_alloc` 0 allocations, 0 reallocations, 0 allocated bytes;
  `dhat-rs` 0 blocks, 0 total bytes.

## Full Integration Profile

The focused tests above intentionally isolate hot loops. To measure the actual client/server stack,
run the ignored integration profile:

```sh
cargo test -p lightyear_tests --features test_utils --test client_server_allocation -- --ignored --test-threads=1 --nocapture
```

This warms up 100 client-to-server `StringMessage` frames, then measures 1,000 more frames through
the real client/server apps, netcode connection, crossbeam IO, link, transport, message
serialization, and message receive systems. Test message construction happens before the measured
region.

Observed on 2026-06-26:

- allocations: `849215`
- reallocations: `5015`
- allocated bytes: `456338748`

This is a diagnostic profile, not a CI budget. It measures the whole app/update stack, including
Bevy schedule work, message string deserialization, netcode, metrics/logging infrastructure, and
transport/link ownership transfers. Use it to spot large regressions or to decide where a DHAT run
would be useful; keep the focused hot-loop tests as the zero-allocation guardrails.

## Known Hot Sites

- Accepted production packets are converted to `Bytes` before upload to `Link`, so their payload
  buffers are not returned to the builder pool.
- `Bytes::from(Vec<u8>)` allocates shared metadata when the vector capacity is larger than its
  length. Avoiding `shrink_to_fit` removes the large resize/copy, but the accepted production send
  path still needs a better ownership model for complete reuse.
- Compression-aware packing clones candidate payloads and compression allocates output buffers.
- Fragment send/reassembly allocates fragment metadata and a contiguous receive buffer.
- Message sender/receiver buffers grow to the largest per-frame burst they have seen. That is a
  cached allocation, but burst sizes above the warm capacity will allocate.
- The full `MessagePlugin::send` production path continues into transport channel buffering. The
  higher-level message serialization fixture stops before transport so it can isolate typed message
  work.
- Prediction histories are pruned in production according to the configured rollback window. Custom
  history buffers or callers that bypass the prediction plugin still need their own retention policy.
- Interpolation histories keep a bracketing pair in steady state. Diff interpolation can retain more
  state while pending diffs are waiting for an older base.
- Prediction correction, interpolation sampling, and frame interpolation clone component values.
  Heap-owning component fields make those clones allocate unless the component uses its own reuse
  strategy.

## Next Steps

1. Decide whether `Link` should accept a reusable payload type, or whether IO backends should write
   from the packet buffer before it is recycled.
2. Add separate budget tests for compression, fragmentation, reliable resend, netcode, and full
   transport-to-link production send.
3. Consider replacing compression candidate cloning with reusable scratch buffers.
4. Add broader but budgeted Bevy schedule allocation tests only after separating expected Bevy
   scheduler/cache allocations from Lightyear hot-path allocations.
