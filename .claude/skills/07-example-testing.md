# Example Testing Skill

## Purpose

Run lightyear examples (like `simple_box`) in client-server or host-server mode, capture logs, and validate that the networking pipeline is working correctly. Since we cannot visually inspect game windows, validation is entirely **log-based**.

## Prerequisites

### Build Dependencies

The `gui` feature requires `bevy_audio` which needs `alsa-lib-devel`. If not installed, create a stub:

```bash
# Create alsa.pc stub if pkg-config can't find alsa
cat > /tmp/alsa.pc << EOF
prefix=/usr
exec_prefix=\${prefix}
libdir=$HOME/.local/lib
includedir=\${prefix}/include
Name: alsa
Description: ALSA
Version: 1.2.14
Libs: -L$HOME/.local/lib -lasound
Cflags: -I\${includedir}
EOF

# Symlink the shared library
mkdir -p $HOME/.local/lib
ln -sf /usr/lib64/libasound.so.2 $HOME/.local/lib/libasound.so

# Then prefix all cargo commands with:
PKG_CONFIG_PATH=/tmp:$PKG_CONFIG_PATH cargo build ...
```

### Display

Examples with GUI need an X11 display (`$DISPLAY` must be set). On headless servers, use `xvfb-run` or `Xvfb`.

## Running Examples

### Client-Server Mode (3 processes)

```bash
EXAMPLE=simple_box
LOG_DIR=/tmp/${EXAMPLE}_logs
mkdir -p $LOG_DIR

# Start headless server (no GUI features)
PKG_CONFIG_PATH=/tmp:$PKG_CONFIG_PATH RUST_LOG=info \
  cargo run -p $EXAMPLE --no-default-features --features=server,netcode,udp -- server \
  > $LOG_DIR/server.log 2>&1 &
SERVER_PID=$!

# Wait for server to start listening
for i in $(seq 1 120); do
  if grep -q "starting at\|Listening\|panicked" $LOG_DIR/server.log 2>/dev/null; then break; fi
  sleep 2
done

# Start client 1
PKG_CONFIG_PATH=/tmp:$PKG_CONFIG_PATH RUST_LOG=info \
  cargo run -p $EXAMPLE --no-default-features --features=netcode,client,udp -- client -c 1 \
  > $LOG_DIR/client1.log 2>&1 &
CL1_PID=$!

# Start client 2
PKG_CONFIG_PATH=/tmp:$PKG_CONFIG_PATH RUST_LOG=info \
  cargo run -p $EXAMPLE --no-default-features --features=netcode,client,udp -- client -c 2 \
  > $LOG_DIR/client2.log 2>&1 &
CL2_PID=$!
```

**Important**: The `client` feature in `simple_box` implies `gui`, which pulls in rendering. To run truly headless clients, the example's Cargo.toml would need a `client` feature without `gui`.

### Host-Server Mode (2 processes)

```bash
# Start host-server (server + host client in same process)
PKG_CONFIG_PATH=/tmp:$PKG_CONFIG_PATH RUST_LOG=info \
  cargo run -p $EXAMPLE --no-default-features --features=server,client,netcode,udp -- host-client -c 0 \
  > $LOG_DIR/host.log 2>&1 &

# Start remote client
PKG_CONFIG_PATH=/tmp:$PKG_CONFIG_PATH RUST_LOG=info \
  cargo run -p $EXAMPLE --no-default-features --features=netcode,client,udp -- client -c 1 \
  > $LOG_DIR/client1.log 2>&1 &
```

### Cleanup

```bash
pkill -f "$EXAMPLE" 2>/dev/null
```

## Log-Based Validation

### What to Check

After starting all processes and waiting for connections (~10-30s including compilation), analyze logs for:

#### 1. Server Health

```bash
# Must see: listener started
grep -i "starting at\|Listening" $LOG_DIR/server.log

# Must see: client connections (one per client)
grep "New connection on netcode" $LOG_DIR/server.log

# Must see: player entities created
grep "Create player entity" $LOG_DIR/server.log

# Must NOT see: panics
grep "panicked" $LOG_DIR/server.log
```

#### 2. Client Connection

```bash
# Must see: client connected
grep "connected" $LOG_DIR/client1.log

# Should see: predicted entities spawned (means replication worked)
grep -i "InputMarker\|Predicted.*entity\|Add.*Predicted" $LOG_DIR/client1.log
```

#### 3. Replication Health

```bash
# Check for replication errors
grep -i "error" $LOG_DIR/server.log | grep -v "wgpu\|vulkan\|cache\|shader\|audio"
grep -i "error" $LOG_DIR/client1.log | grep -v "wgpu\|vulkan\|cache\|shader\|audio"

# Known issues on cb/lightyear-replicon branch:
# - "Serde Deserialization Error" — protocol not fully adapted for replicon
# - "mapping X to Y, but it's already mapped to Z" — entity map sync issue
```

#### 4. Process Stability

```bash
# All processes should still be alive after 10+ seconds
for pid in $SERVER_PID $CL1_PID $CL2_PID; do
  kill -0 $pid 2>/dev/null && echo "PID $pid: alive" || echo "PID $pid: DEAD"
done
```

### Validation Checklist

| Check | Log Pattern | Severity |
|-------|------------|----------|
| Server started | `starting at` or `Listening` | CRITICAL |
| No server panic | absence of `panicked` | CRITICAL |
| Clients connected | `connected` in client logs | CRITICAL |
| Player entities created | `Create player entity` in server log | HIGH |
| No deserialization errors | absence of `Deserialization Error` | HIGH |
| No entity mapping errors | absence of `already mapped` | MEDIUM |
| Predicted entities spawned | `InputMarker` or `Predicted` in client logs | HIGH |
| Processes alive after 10s | `kill -0` succeeds | CRITICAL |

### Severity Levels

- **CRITICAL**: Test fails. The example is fundamentally broken.
- **HIGH**: Replication/prediction not working. Networking connects but gameplay is broken.
- **MEDIUM**: Known issues, likely from ongoing migration work.

## Current Status (cb/lightyear-replicon branch)

As of March 2026, `simple_box` in client-server mode:
- Server starts and listens ✅
- Both clients connect via netcode ✅
- Server creates player entities ✅
- All processes stay alive ✅
- Clients receive replication data ❌ (Serde Deserialization Error)
- Entity mapping has conflicts ❌ (already mapped error)
- Predicted entities spawn on clients ❌ (blocked by deserialization)

The example's protocol registration needs updating for replicon's serialization format.

## Known Build Issues

1. **`SendUpdatesMode` doesn't exist**: The old `ReplicationSender::new(interval, mode, flag)` API was removed. Replace with `ReplicationSender::default()`. This affects most examples.

2. **`alsa-sys` build failure**: Missing `alsa-lib-devel`. Use the stub workaround above.

3. **`ServerMutateTicks` resource missing**: If running server-only (no `client` feature), prediction systems panic because `ServerMutateTicks` is a client-side replicon resource. Workaround: include the `client` feature even for server-only binaries, or gate prediction systems behind a client feature flag.

## Example List

| Example | Directory | Notes |
|---------|-----------|-------|
| `simple_box` | `examples/simple_box/` | Basic 2D box movement, prediction + interpolation |
| `simple_setup` | `examples/simple_setup/` | Minimal setup |
| `bevy_enhanced_inputs` | `examples/bevy_enhanced_inputs/` | BEI input integration |
| `client_replication` | `examples/client_replication/` | Client-authoritative replication |
| `distributed_authority` | `examples/distributed_authority/` | Authority transfer |
| `replication_groups` | `examples/replication_groups/` | Grouped entity replication |
| `network_visibility` | `examples/network_visibility/` | Per-client visibility |
| `priority` | `examples/priority/` | Bandwidth priority |
| `delta_compression` | `examples/delta_compression/` | Delta compression |
| `avian_physics` | `examples/avian_physics/` | 2D physics |
| `fps` | `examples/fps/` | 3D first-person |
| `lobby` | `examples/lobby/` | Lobby/matchmaking |
| `projectiles` | `examples/projectiles/` | Projectile prediction |
