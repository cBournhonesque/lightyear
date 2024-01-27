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


To start the server, run `cargo run -- server`

Then you can launch multiple clients with the commands:

- `cargo run -- client -c 1`

- `cargo run -- client -c 2 --client-port 2000`
