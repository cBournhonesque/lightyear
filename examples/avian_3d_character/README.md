# Avian 3D Character

This is an example of a server containing server-authoritative, physics-based, 3D characters simulated with `avian3d` and clients controlling those characters and predicting their movement.

## Features

* The client will immediately try to connect to the server on start.
* The server will spawn a new character for each client that connects and give that client control over the character.
  * A character is a dynamic 3D capsule.
  * The client can control the character with `W/A/S/D/SPACE`.
  * Client inputs are converted into physical forces applied to the character.
  * All clients will predict the position, rotation, and velocity of all characters.
* The serve will spawn some dynamic blocks and a static floor on start.
  * All clients will predict the position, rotation, and velocity of all blocks.
  * The floor is only replicated and not predicted because we do not expect it to move.

## Running the example

- Run the server: `cargo run server`
- Run client with id 1: `cargo run client -c 1`
- Run client with id 2: `cargo run client -c 2` (etc.)

You can modify the file `assets/settings.ron` to modify some networking settings.
