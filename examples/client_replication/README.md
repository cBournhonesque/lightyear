# Introduction

A simple example that shows how to use lightyear for client-replication (the entity is spawned on the client and replicated to the server and then to other clients):
  - with client-authority: the circle cursor is replicated to the server and to other clients. Any client updates are replicated to the server.
    The client's cursor is replicated to the server; one just needs to add the `Replicate` component on the server to then
    replicate the cursor to other clients. 
  
  - with server-authority: when pressing space, a square is spawned on the client. That square is a 'pre-predicted' entity:
    it will get replicated to the server. The server can replicate it back to all clients.
    When the original client gets the square back, it will spawn a 'Confirmed' square on the client, and will recognize
    that the original square spawned was a prediction. From there on it's normal replication.


## Running the example

To start the server, run `cargo run --example client_replication server`

Then you can launch multiple clients with the commands:

- `cargo run --example client_replication client -c 1`

- `cargo run --example client_replication client -c 2 --client-port 2000`
