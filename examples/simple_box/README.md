# Simple box

A simple example that shows how to use Lightyear to create a server-authoritative multiplayer game.

It also showcases how to enable client-side prediction and snapshot interpolation:
- For the client sending inputs: the pink cube is client-predicted (so inputs are used with no delay, and there is a rollback in case of mismatch with the server) and the red cube shows the received server state. (the server state arrives with some delay, and is a bit choppy since the replication rate is only 10Hz).
- For the other clients: the red cube still shows the server states arriving at 10Hz, and the pink cube is a smooth interpolation between those states (there is a slight delay because we can only interpolate between 2 received server states).

https://github.com/cBournhonesque/lightyear/assets/8112632/7b57d48a-d8b0-4cdd-a16f-f991a394c852

## Running an example

- Run the server with a GUI: `cargo run -- --headless=false server`
- Run client with id 1: `cargo run -- client -c 1`

[//]: # (- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`)
- Run the server without a gui: `cargo run --no-default-features --features=server -- server`
- Run a headless client without a gui: `cargo run --no-default-features --features=client,netcode,webtransport -- client -c 1`
- Run the client and server in "HostClient" mode, where the client also acts as server (both are in the same App) : `cargo run -- host-client -c 0`

### P2P mode

The same example can run as a deterministic, input-only P2P game with no server or authoritative
simulation. Peers discover each other through a lobby instead of a preconfigured roster:

- First peer (opens the lobby): `cargo run --no-default-features --features=p2p -- --headless=true p2p --port 6100`
- Each further peer, pointing at any peer that is already running: `cargo run --no-default-features --features=p2p -- --headless=true p2p --port 6101 --peer 127.0.0.1:6100`

Every peer opens an endpoint on its `--port` that other peers can connect to, so peers on the same
machine each need their own port. A joining peer only needs the address of one peer that is already
started and discovers the rest of the roster through the lobby. The game starts on all peers as soon
as the founding player count is reached (2 by default, override with `-n` on every peer).

P2P mode supports two through four players.
Every peer spawns the same founding roster with stable, peer-derived `PreSpawned` hashes, simulates every
player locally, and sends only its own tick-indexed inputs to the other peers. Each peer predicts
missing remote inputs by repeating the latest known input, then rolls back and replays the complete
deterministic world when corrected input arrives. The peers wait until their discovered Links and
the input timeline are ready, then acknowledge a shared future start tick. The normal client/server and host-client modes remain
available in the same example.

For sequential joins, start the two founders above, then run peer 2 with `LIGHTYEAR_P2P_JOIN=1`.
The `--peer` seed is also its admission/bootstrap peer. After peer 2 activates, start peer 3 through
peer 2 to exercise catch-up from a previously joined peer:

```shell
# Peer 2 joins through founder 0; its lower port also exercises changing lobby sort order.
LIGHTYEAR_P2P_JOIN=1 cargo run --no-default-features --features=p2p -- --headless=true p2p --port 6099 --peer 127.0.0.1:6100 -n 2
# Wait for peer 2 to activate, then peer 3 joins through it.
LIGHTYEAR_P2P_JOIN=1 cargo run --no-default-features --features=p2p -- --headless=true p2p --port 6102 --peer 127.0.0.1:6099 -n 2
```

Use the same `-n` founding count on every process; it is not a predeclared final roster.
Set `LIGHTYEAR_SIMPLE_BOX_AUTOMOVE=random:1` (a different seed per peer) and
`LIGHTYEAR_SIMPLE_BOX_RANDOM_INTERVAL_TICKS=1` to exercise continuously changing inputs.

Applications select a founding roster with `P2PStart { cohort: NetworkTarget::Only(peers) }`;
`P2PStart::default()` selects all declared inactive Links. A running session admits a newcomer
through `P2PJoinRequested` and an application-triggered `P2PJoinAdmission` reply. Once catch-up
finishes, all peers agree one future activation tick; create the new player on `P2PJoined`.

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,webtransport,netcode`.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

The repo includes a pre-generated self-signed WebTransport certificate and digest, so you do not need to run the certificate generator for the usual local workflow while that certificate is valid. If it expires, or if you want to replace it, generate a new temporary self-signed certificate with:
- `cargo run -p generate_certificate` (writes `certificates/cert.pem`, `certificates/key.pem`, and `certificates/digest.txt`; rebuild wasm clients after regenerating so they embed the new digest)
