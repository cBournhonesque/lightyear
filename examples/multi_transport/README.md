# Multi transport

A simple example that shows how a Lightyear server can communicate with clients using different transports.
This means that there is cross-play possibilities between different connection layers (Steam, WebTransport, etc.)

The example uses both the default UDP transport and the WebTransport transport.

## Running the example

To start the server, run `cargo run -- server`

Then you can launch multiple clients with the commands:

- `cargo run -- client -c 1`

- `cargo run -- client -c 2 --client-port 2000`

To use webtransport:
- `cargo run -- server --transport web-transport`
- `cargo run -- client -c 1 --transport web-transport`