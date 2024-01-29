# Introduction

A simple example that shows how to use lightyear for client-replication (the entity is spawned on the client and replicated to the server):
  - with client-authority: the cursor is replicated to the server and to other clients. Any client updates are replicated to the server.
    If we want to replicate it to other clients, we just needs to add the `Replicate` component on the server's entity to replicate the cursor to other clients.
  
  - spawning pre-predicted entities on the client: when pressing the `Space` key, a square is spawned on the client. That square is a 'pre-predicted' entity:
    it will get replicated to the server. The server can replicate it back to all clients.
    When the original client gets the square back, it will spawn a 'Confirmed' square on the client, and will recognize
    that the original square spawned was a prediction. From there on it's normal replication.

  - pressing `M` will send a message from a client to other clients

  - pressing `K` will delete the Predicted entity. You can use this to confirm various rollback edge-cases.


https://github.com/cBournhonesque/lightyear/assets/8112632/718bfa44-80b5-4d83-a360-aae076f81fc3


## Running the example

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.


To start the server, run `cargo run -- server`

Then you can launch multiple clients with the commands:

- `cargo run -- client -c 1`

- `cargo run -- client -c 2 --client-port 2000`


### Testing in wasm


NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

To test the example in wasm, you can run the following commands:
- `sh examples/generate.sh` (to generate the temporary SSL certificates, they are only valid for 2 weeks)
- `cargo run -- server --transport web-transport` to start the server
- You will then need to copy the certificate digest string that is outputted by the server in the logs and paste it in the `examples/interest_management/client.rs` file.
  Replace the certificate value like so:
```
let certificate_digest =
String::from("09945594ec0978bb76891fb5de82106d7928191152777c9fc81bec0406055159");
```
- then start the client wasm test with `trunk serve`

NOTE:
- the wasm example seems to work better in release mode!
