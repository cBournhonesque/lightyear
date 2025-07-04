# Features

This example showcases several things:

- how to integrate lightyear with `leafwing_input_manager`. In particular you can simply attach an `ActionState` and
  an `InputMap`
  to an `Entity`, and the `ActionState` for that `Entity` will be replicated automatically
- an example of how to integrate physics replication with `bevy_xpbd`. The physics sets have to be run in `FixedUpdate`
  schedule
- an example of how to run prediction for entities that are controlled by other players. (this is similar to what
  RocketLeague does).
  There is going to be a frequent number of mispredictions because the client is predicting other players without
  knowing their inputs.
  The client will just consider that other players are doing the same thing as the last time it received their inputs.
  You can use the parameter `--predict` on the server to enable this behaviour (if not, other players will be
  interpolated).
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
For example you can run the server in headless mode (without gui) by running `cargo run --no-default-features --features=server,udp,netcode`.

### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands to generate a self-signed certificate:
- `cd "$(git rev-parse --show-toplevel)" && sh certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
