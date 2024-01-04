# Features

- multiple entities with a leafwing ActionState (i.e. 2 entities are controlled by action state)
- a global action-state (with a different ActionLike) for chat
- chat displayed on screen
- each player controls 2 cubes (with either WASD or arrows) and there is a ball
- they can score


# Usage

- Run the server with: `cargo run --example leafwing_inputs --features leafwing -- server`
- Run the clients with:
`cargo run --example leafwing_inputs --features leafwing -- client -c 1`
`cargo run --example leafwing_inputs --features leafwing -- client -c 2`
