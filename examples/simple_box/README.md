# Simple box

A simple example that shows how to use Lightyear to create a server-authoritative multiplayer game.

It also showcases how to enable client-side prediction and snapshot interpolation.


## Running the example

To start the server, run `cargo run --example simple_box server`

Then you can launch multiple clients with the commands:

- `cargo run --example simple_box client -c 1`

- `cargo run --example simple_box client -c 2 --client-port 2000`
