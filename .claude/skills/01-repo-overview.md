# Lightyear Repository Overview

## What Is Lightyear?

Lightyear is a **modular server-client networking library for the Bevy game engine** (version 0.26.4). It provides replication, prediction, interpolation, and input handling for multiplayer games. The codebase is split into ~34 workspace crates organized in layers.

## Architecture Layers (Bottom to Top)

```
Application Layer     (user game code)
    |
Input Layer           lightyear_inputs, lightyear_inputs_native, lightyear_inputs_bei, lightyear_inputs_leafwing
    |
Prediction Layer      lightyear_prediction, lightyear_deterministic_replication
    |
Sync/State Layer      lightyear_sync, lightyear_replication, lightyear_interpolation, lightyear_frame_interpolation
    |
Message Layer         lightyear_messages, lightyear_serde
    |
Transport Layer       lightyear_transport (packet fragmentation, channels, reliability)
    |
Connection Layer      lightyear_connection, lightyear_netcode, lightyear_raw_connection, lightyear_steam
    |
IO Layer              lightyear_link, lightyear_udp, lightyear_websocket, lightyear_webtransport, lightyear_crossbeam, lightyear_aeronet
```

## Crate Summary

### Core
| Crate | Purpose |
|-------|---------|
| `lightyear` | Main wrapper crate. Provides `ClientPlugins` and `ServerPlugins` PluginGroups |
| `lightyear_core` | Fundamental types: Tick, PeerId, timelines, history buffers |
| `lightyear_utils` | Data structures: free lists, sequence buffers, wrapping IDs, metrics |

### IO Layer
| Crate | Purpose |
|-------|---------|
| `lightyear_link` | Transport-agnostic `Link` component for buffering bytes, link conditioning |
| `lightyear_udp` | UDP transport via `std::net::UdpSocket` |
| `lightyear_websocket` | WebSocket transport via aeronet |
| `lightyear_webtransport` | WebTransport protocol via aeronet |
| `lightyear_crossbeam` | In-process channel transport (used extensively in tests) |
| `lightyear_aeronet` | Aeronet session wrapper for Lightyear's Link |

### Connection Layer
| Crate | Purpose |
|-------|---------|
| `lightyear_connection` | Long-term connection management, PeerId, Client/Server components |
| `lightyear_netcode` | netcode.io protocol: cryptographic auth, connect tokens |
| `lightyear_raw_connection` | Simple connection where Link = Connection (no protocol layer) |
| `lightyear_steam` | Steam networking sockets integration |

### Packet & Message
| Crate | Purpose |
|-------|---------|
| `lightyear_transport` | Packet fragmentation, channels (reliable/unreliable/sequenced), flow control |
| `lightyear_serde` | Network serialization: `ToBytes` trait, varint encoding, entity mapping |
| `lightyear_messages` | High-level `MessageSender<M>` / `MessageReceiver<M>` components |

### Sync & State
| Crate | Purpose |
|-------|---------|
| `lightyear_sync` | Ping/RTT estimation, timeline synchronization |
| `lightyear_replication` | Entity/component replication via bevy_replicon, visibility, authority, control |
| `lightyear_interpolation` | Client-side interpolation between server updates |
| `lightyear_frame_interpolation` | Visual interpolation between FixedUpdate ticks |

### Prediction
| Crate | Purpose |
|-------|---------|
| `lightyear_prediction` | Client-side prediction, rollback, visual correction |
| `lightyear_deterministic_replication` | Input-only replication with checksum validation |

### Input
| Crate | Purpose |
|-------|---------|
| `lightyear_inputs` | Core input infrastructure: history buffer, input channels |
| `lightyear_inputs_native` | User-defined input structs |
| `lightyear_inputs_leafwing` | leafwing-input-manager integration |
| `lightyear_inputs_bei` | bevy_enhanced_input integration |

### Physics & Debug
| Crate | Purpose |
|-------|---------|
| `lightyear_avian2d` / `lightyear_avian3d` | Avian physics integration with lag compensation |
| `lightyear_metrics` | Performance metrics collection |
| `lightyear_ui` | Runtime debug UI |
| `lightyear_web` | WASM/web support |
| `lightyear_tests` | Integration test infrastructure |

## Key Entry Points for Understanding the Code

- **Replication logic**: `lightyear_replication/src/send.rs` (Replicate, PredictionTarget, InterpolationTarget hooks)
- **Prediction/rollback**: `lightyear_prediction/src/rollback.rs` and `predicted_history.rs`
- **Connection setup**: `lightyear_connection/src/` (client.rs, server.rs, host.rs)
- **Transport channels**: `lightyear_transport/src/channel/`
- **Test harness**: `lightyear_tests/src/stepper.rs`

## Current Branch: `cb/lightyear-replicon`

This is an active integration branch migrating the replication backend from a custom system to bevy_replicon. See `02-replicon-migration.md` for details.
