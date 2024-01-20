# Features


This example showcases how prespawning player objects on the client side works:
- you just have to add a `PreSpawnedPlayedObject` component to the pre-spawned entity. The system that spawns the entity can be identical in the client and the server
- the client spawns the entity immediately in the predicted timeline
- when the client receives the server entity, it will match it with the existing pre-spawned entity!



https://github.com/cBournhonesque/lightyear/assets/8112632/ee547c32-1f14-4bdc-9e6d-67f900af84d0



# Usage

- Run the server with: `cargo run -- server --headless`
- Run the clients with:
`cargo run -- client -c 1`
`cargo run -- client -c 2`
