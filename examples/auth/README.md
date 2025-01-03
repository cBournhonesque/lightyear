# Auth with ConnectTokens

This example will showcase how to get a `ConnectToken` to establish a connection with a game server.

You can find more information in the [book](https://cbournhonesque.github.io/lightyear/book/concepts/connection/title.html#netcode), but basically
you need a `ConnectToken` to establish a connection with a server using the Netcode protocol. (it is not required if you are using 
a different means to establish a connection, such as steam sockets)

The `ConnectToken` is a token that is generated by a backend server, using a private key that is shared with your game servers.
It is sent to the client using a secure method of your choice (TCP+TLS, websockets, HTTPS, etc.).
Once the client has received the `ConnectToken`, they can use it to establish a connection with the game server.

In this example, the game server and the backend will run in the same process. The server will run a separate task
that listens on a TCP socket for incoming requests. For every request, it will generate a `ConnectToken` that it will send
to the client. The client can then use the `ConnectToken` to start the `lightyear` connection.


## Running the example

- Run the server: `cargo run --features=server`
- Run client with id 1: `cargo run --features=client -- -c 1`
- Run client with id 2: `cargo run --features=client -- -c 2` (etc.)
- Run the client and server in two separate bevy Apps: `cargo run --features=server,client`
- Run the server with a gui: `cargo run --features=server,gui`
- Run the client and server in "HostServer" mode, where the server is also a client (there is only one App): `cargo run --features=server,client -- -m=host-server`

You can modify the file `assets/settings.ron` to modify some networking settings.

### Testing in wasm with webtransport

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

You will need a valid SSL certificate to test the example in wasm using webtransport. You will need to run the following
commands:
- `cd "$(git rev-parse --show-toplevel)" && sh examples/certificates/generate.sh` (to generate the temporary SSL
  certificates, they are only valid for 2 weeks)
- Start the server with: `cargo run -- server`
- Then start the wasm client wasm with `trunk serve --features=client`