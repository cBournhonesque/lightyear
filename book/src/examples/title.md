# Examples

The examples are the best way to see how the pieces fit together. Some of them are also used as development test beds, so if an example talks about an experimental feature, read its README before treating it as the recommended path.

Most examples can be run natively, and several have browser builds using WASM and WebTransport. Browser support varies; Safari is usually the awkward one.

### [Simple Box](https://cbournhonesque.github.io/lightyear/examples/simple_box/dist/)

The smallest useful example: connect a client, spawn a server-owned player entity, replicate it, send inputs, and add prediction/interpolation.

This is the example to read first.

### [Replication Groups](https://cbournhonesque.github.io/lightyear/examples/replication_groups/dist/)

Shows more complicated replicated state and entity references. It is useful when one entity's component points at another entity and you need entity mapping to work across the network.

### [Interest Management](https://cbournhonesque.github.io/lightyear/examples/interest_management/dist/)

Shows how to replicate only the entities that are relevant to each player. This is the example to look at when you care about rooms, visibility, or fog-of-war style filtering.

### Client Replication

Treat the client replication example as experimental with the current Replicon backend. The supported entity replication path is server to client; clients should send inputs or messages and let the server replicate the result.

### Bullet Pre-spawn

Shows the latency problem that prespawning tries to solve: a player wants to see a projectile immediately, but the server still needs to own the real replicated entity.

Prespawn matching is still being adapted to the Replicon backend, so use this as a reference for development rather than as the first thing to copy into a game.

### Leafwing Input Prediction

Shows integration with `leafwing_input_manager`, prediction, and physics-style movement. It is useful when your input model is larger than a tiny enum.

### [Priority](https://cbournhonesque.github.io/lightyear/examples/priority/dist/)

Shows bandwidth-pressure ideas. Verify the details against the current Replicon-backed replication path before copying them into a game.
