# Features


This example showcases the different types of spawning entities:
- normal: the entity will get spawned on the server, and then will be replicated to clients.
  If prediction or interpolation is enabled, 2 entities will be created on the client instead of one: a Confirmed entity and a Predicted/Interpolated entity.
- pre-spawned user: the entity is first spawned on the client. 
  By adding the component `ShouldBePredicted` on the client for that entity, you indicate that this is a user-controlled pre-spawned predicted entity.
  You have to manually replicate the entity to the server (for now).
  On the server, you will need to replicate the entity back to the client and enable prediction.
  The server will get authority over that entity, but the user inputs can get applied **immediately** for that entity
- pre-spawned objects: setting up pre-spawned user is somewhat unwieldy because the client needs to send a replication message to the server
  to let the server know that the entity is player-controlled and pre-spawned.




https://github.com/cBournhonesque/lightyear/assets/8112632/ac6fb465-26b8-4f5b-b22b-d79d0f48f7dd

*Example with 150ms of simulated RTT, a 32Hz server replication rate, 7 ticks of input-delay, and rollback-corrections enabled.*



# Usage

- Run the server with: `cargo run -- server --predict`
- Run the clients with:
`cargo run -- client -c 1`
`cargo run -- client -c 2`
