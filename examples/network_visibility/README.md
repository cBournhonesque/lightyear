# Interest management

A simple example that shows how to use Lightyear to perform interest management.

Interest management is a technique to reduce the amount of data that is sent to each client:
we want to send only the data that is relevant to each client.

In this example, we are going to replicate entities that are within a certain distance of the client.

## Visibility lifetimes

The client renders three kinds of circles so the lifetime policies can be compared directly:

| Circle | Server policy | What the client observes |
| --- | --- | --- |
| Small green | `lose_visibility` / `WhileVisible` | Despawns outside the interest radius and respawns when it returns. |
| Medium red | `lose_visibility_retained` / `AfterFirstVisibility` | Starts inside the interest radius. After you move away, it remains at its last received state while network updates are paused. |
| Large blue | `lose_visibility_always_present` / `AlwaysPresent` | The server marks it hidden before its first replication, but the client still receives its initial state. Later updates remain paused while it is hidden. |

The player starts at the origin. Move with WASD or the arrow keys; moving more than the interest
radius away from the red circle demonstrates that it remains on the client while ordinary green
circles disappear. The blue circle appears immediately even though the server never gains
visibility for it.

Retaining an entity is useful when destroying and rebuilding its client-side state would be costly,
when other client entities need a stable reference to it, or when the UI should preserve a last
known state after an entity leaves interest. `AlwaysPresent` is useful for identities, hierarchy
roots, roster entries, objectives, or placeholders that must exist on every client even before
their live state becomes relevant. Because `AlwaysPresent` reveals the entity's existence and
initial state, it is not appropriate for information that must remain secret while hidden.

Retained entities do not receive mutations while hidden, and the client is not automatically told
that they became hidden. Games that need a stale/dormant indicator or need to suspend prediction
should communicate that separately. A real server despawn still despawns retained client entities;
remove `Replicate` before despawning when the client copy should intentionally survive.

https://github.com/cBournhonesque/lightyear/assets/8112632/41a6d102-77a1-4a44-8974-1d208b4ef798

## Running an example

- Run the server with a GUI: `cargo run -- --headless=false server`
- Run client with id 1: `cargo run -- client -c 1`

[//]: # (- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`)
- Run the server without a gui: `cargo run --no-default-features --features=server -- server`
- Run the client and server in "HostClient" mode, where the client also acts as server (both are in the same App) : `cargo run -- host-client -c 0`

### P2P mode

The movement simulation can also run as a deterministic input-only game with no server. Start
peer 0 with `cargo run --no-default-features --features=p2p -- --headless=true p2p --peer-id 0 --player-count 2`
and peer 1 with the same command using `--peer-id 1`. Every peer creates the complete small scene
locally. Interest management remains a feature of the conventional client/server mode because the
P2P mode performs no entity replication.

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,webtransport,netcode`.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

The repo includes a pre-generated self-signed WebTransport certificate and digest, so you do not need to run the certificate generator for the usual local workflow while that certificate is valid. If it expires, or if you want to replace it, generate a new temporary self-signed certificate with:
- `cargo run -p generate_certificate` (writes `certificates/cert.pem`, `certificates/key.pem`, and `certificates/digest.txt`; rebuild wasm clients after regenerating so they embed the new digest)
