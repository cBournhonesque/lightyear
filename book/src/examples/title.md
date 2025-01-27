# Examples


This page contains a list of examples that you can run to see how Lightyear works.
Click on the links to run the example in your browser.
The examples run using WASM and WebTransport, so they might not work on all browsers (for example, they don't work on Safari currently).

### [Simple Box](https://cbournhonesque.github.io/lightyear/examples/simple_box/dist/)

Simple example showing how to replicate a box between the client and server.

Two boxes are actually shown:
- the red box is the Confirmed entity, which is updated at every server update (i.e. every 100ms)
- the pink box is either:
  - the Predicted entity (if the client is controlling the entity)
  - the Interpolated entity (if the client is not controlling the entity): the entity's movements are smoothed between two server updates


### [Replication Groups](https://cbournhonesque.github.io/lightyear/examples/replication_groups/dist/)

This is an example that shows how to make Lightyear replicate multiple entities in a single message,
to make sure that they are always in a consistent state (i.e. that entities in a group are all replicated on the same tick).

It also shows how lightyear can replicate components that are references to an entity. Lightyear will take care of mapping the 
entity from the server's `World` to the client's `World`.

### [Interest Management](https://cbournhonesque.github.io/lightyear/examples/interest_management/dist/)

This example shows how lightyear can perform interest management: replicate only a subset of entities to each player.
Here, the server will only replicate the green dots that are close to each player.


### [Client Replication](https://cbournhonesque.github.io/lightyear/examples/client_replication/dist/)

This example shows how lightyear can be used to replicate entities from the client to the server (and to other clients).
The replication can be client-controlled.

It also shows how to spawn entities directly on the client's predicted timeline.

### [FPS](https://cbournhonesque.github.io/lightyear/examples/fps/dist/)

This example shows how to easily pre-spawn entities on the client's predicted timeline.
The bullets are created using the same system on both client and server; however when the server
replicates a bullet to the client, the client will match it with the existing pre-spawned bullet (instead of creating a new entity).

It also showcases how to use lag compensation to compute collisions between predicted and interpolated entities.

### [Leafwing Input Prediction](https://cbournhonesque.github.io/lightyear/examples/leafwing_inputs/dist/)

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

### [Priority](https://cbournhonesque.github.io/lightyear/examples/priority/dist/)

This examples shows how `lightyear` can help with bandwidth management.
See this [chapter](https://cbournhonesque.github.io/lightyear/book/concepts/advanced_replication/bandwidth_management.html) of the book.

Lightyear can limit the bandwidth used by the client or the server, for example to limit server traffic costs, or because the client's connection cannot handle a very high bandwidth.
You can then assign a **priority** score to indicate which entities/messages are important and should be sent first.

In this example, the middle row has a priority of 1.0, and the priority increases by 1.0 for each row further away from the center.
(i.e. the edge rows have a priority of 7.0 and are updated 7 times more frequently than the center row)
