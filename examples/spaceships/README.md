# Spaceships Demo

Based on the `xpbd_demo` but everything is predicted, server authoritative spawning of players,
movement using physics forces instead of directly setting velocities.


# Example scenario when replicating inputs immediately makes sense

* two clients connected, A and B. Each is 20ms away from the server.
* fixed update is 15ms per tick. (approx 64hz)
* both clients have a 3 tick input delay. (assume this is a decent compromise on a low-latency client between things feeling snappy enough locally, and players with higher latencies not having to rollback loads for every update and experiencing too much jank.)
* let's say updates from the server for tick N arrive at the clients while they are simulating N+3.

client A simulates tick 10, user presses Fire.
due to input delay of 3, this is immediately transmitted to the server as "[A] fire=true @ tick 13"
when simulating frame 13, client A will prespawn their own bullet based on their ActionState=fire for that tick.
(when the server simulates tick 13, it will spawn a bullet, which is then replicated back and ultimately merged with our predicted one)

A's inputs for tick 13 (sent on A's tick 10) take 20ms to reach the server, then 20ms to reach client B. 
A & B have the same input delay and latency, so should be simulating roughly the same tick at the same time.
20ms + 20ms = 40ms / 15ms = inputs are in-flight between A->server->B for 2.7 ticks.

So B will receive A's inputs for tick 13 when it is at tick 12.7, ie just in time.

so on the B client, at the start of tick 13, the ActionState component for remote player A's predicted entity will be set to fire=true, but because B is simulating in the future compared to the server, there will not yet be a server update with a replicated bullet spawn message.

### With prespawning 
at B's tick 13 we can spawn the predicted bullet on behalf of A, based on their ActionState. In theory this should not require any rollbacks when the server also spawns the bullet on the server's tick 13, and replicates it to B, which will receive it around B's tick 16 - check history, decide no rollback needed.

### Without prespawning
alternatively if we don't spawn the bullet based on the actionstate, around when B simulates tick 16 it will receive server updates for the bullet spawn that happened on tick 13. B will then have to rollback to 13, spawn the bullet, fast-forward back to 16. The bullet will appear to spawn 3-ticks ahead of where it was fired from.

### Issues

Tracking and merging predicted with server-replicated entities already works when it's your own bullet, because you will always have spawned the predicted entity with `PreSpawnedPlayerObject` BEFORE the server spawns and replicates the bullet to you.





## ----

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

## Running the example

There are different 'modes' of operation:

- as a dedicated server with `cargo run -- server`
- as a listen server with `cargo run -- listen-server`. This will launch 2 independent bevy apps (client and server) in
  separate threads.
  They will communicate via channels (so with almost 0 latency)
- as a listen server with `cargo run -- host-server`. This will launch a single bevy app, where the server will also act
  as a client. Functionally, it is similar to the "listen-server" mode, but you have a single bevy `World` instead of
  separate client and server `Worlds`s.

Then you can launch clients with the commands:

- `cargo run -- client -c 1` (`-c 1` overrides the client id, to use client id 1)
- `cargo run -- client -c 2`

You can modify the file `assets/settings.ron` to modify some networking settings.

### Testing in wasm with webtransport

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `trunk serve`

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands:

- `sh examples/generate.sh` (to generate the temporary SSL certificates, they are only valid for 2 weeks)
- `cargo run -- server` to start the server. The server will print out the certificate digest (something
  like `1fd28860bd2010067cee636a64bcbb492142295b297fd8c480e604b70ce4d644`)
- You then have to replace the certificate digest in the `assets/settings.ron` file with the one that the server printed
  out.
- then start the client wasm test with `trunk serve`