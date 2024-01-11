# Features

This example showcases several things:
- how to integrate lightyear with `leafwing_input_manager`. In particular you can simply attach an `ActionState` and an `InputMap`
  to an `Entity`, and the `ActionState` for that `Entity` will be replicated automatically
- an example of how to integrate physics replication with `bevy_xpbd`. The physics sets have to be run in `FixedUpdateSet::Main`
- an example of how to run prediction for entities that are controlled by other players. (this is similar to what RocketLeague does).
  There is going to be a frequent number of mispredictions because the client is predicting other players without knowing their inputs.
  The client will just consider that other players are doing the same thing as the last time it received their inputs.
  You can use the parameter `--predict` on the server to enable this behaviour (if not, other players will be interpolated).
- The prediction behaviour can be adjusted by two parameters:
  - `input_delay`: the number of frames it will take for an input to be executed. If the input delay is greater than the RTT,
     there should be no mispredictions at all, but the game will feel more laggy.
  - `correction_ticks`: when there is a misprediction, we don't immediately snapback to the corrected state, but instead we visually interpolate
    from the current state to the corrected state. This parameter helps make mispredictions less jittery.


# Usage

- Run the server with: `cargo run -- server --predict`
- Run the clients with:
`cargo run -- client -c 1`
`cargo run -- client -c 2`
