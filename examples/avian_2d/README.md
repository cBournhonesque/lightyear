# Features

This example showcases several things:

- how to integrate lightyear with `leafwing_input_manager`. In particular you can simply attach an `ActionState` and
  an `InputMap`
  to an `Entity`, and the `ActionState` for that `Entity` will be replicated automatically
- an example of how to integrate physics replication with `avian2d`. The physics sets have to be run in `FixedUpdate`
  schedule
- an example of predicting all dynamically interacting players on every client. Remote inputs are rebroadcast for
  prediction, while each client's keyboard still controls only its own player.
- compound collider transform propagation: each player rigid body has a smaller child cube collider immediately beside
  and touching the main cube, with no rigid body of its own. Its local transform remains at a fixed offset while its
  world position and rotation follow the player. The child is deterministic template data: an `On<Add, PlayerId>`
  observer constructs it locally on the server and every client. Both colliders are one physical player
  body: contacts on the smaller cube apply forces to the main rigid-body root, and player input is applied only to that
  root.

The child entity and its components are not replicated. `DisableReplicateHierarchy` on the player root prevents its
locally spawned child from automatically inheriting `ReplicateLike`; adding a separate `Replicate` component is unnecessary.
The root's predicted/interpolated pose plus the child's fixed local `Transform` are the authoritative state. Bevy transform
propagation produces `GlobalTransform`, and the Avian integration derives the collider's world-space physics pose from
that hierarchy, so no additional application-level `Transform`/`Position` sync system is needed. Replicating a second
world pose for the child would create a redundant timeline that can disagree with
the root during rollback, correction, or interpolation. A child with its own `RigidBody` would be different: it would
have independent physics state and should replicate/predict its own pose.

- The prediction behaviour can be adjusted by two parameters:
    - `input_delay`: the number of frames it will take for an input to be executed. If the input delay is greater than
      the RTT,
      there should be no mispredictions at all, but the game will feel more laggy.
    - `correction_ticks`: when there is a misprediction, we don't immediately snapback to the corrected state, but
      instead we visually interpolate
      from the current state to the corrected state. This parameter helps make mispredictions less jittery.

https://github.com/cBournhonesque/lightyear/assets/8112632/ac6fb465-26b8-4f5b-b22b-d79d0f48f7dd

*Example with 150ms of simulated RTT, a 32Hz server replication rate, 7 ticks of input-delay, and rollback-corrections
enabled.*

## Running an example

- Run the server with a gui: `cargo run -- server`
- Run client with id 1: `cargo run -- client -c 1`

[//]: # (- Run the client and server in two separate bevy Apps: `cargo run` or `cargo run separate`)
- Run the server without a gui: `cargo run --no-default-features --features=server -- server`
- Run the client and server in "HostClient" mode, where the client also acts as server (both are in the same App) : `cargo run -- host-client -c 0`

You can control the behaviour of the example by changing the list of features. By default, all features are enabled (client, server, gui).
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,webtransport,netcode`.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

The repo includes a pre-generated self-signed WebTransport certificate and digest, so `certificates/generate.sh` is not required for the usual local workflow while that certificate is valid. If it expires, or if you want to replace it, generate a new temporary self-signed certificate with:
- `cd "$(git rev-parse --show-toplevel)" && sh certificates/generate.sh` (writes `certificates/cert.pem`, `certificates/key.pem`, and `certificates/digest.txt`; rebuild wasm clients after regenerating so they embed the new digest)
