# Introduction

A simple example that shows how to use lightyear for replication:
- server-replication: the entity is spawned on the server and replicated to the clients
- client-replication: the entity is spawned on the client and replicated to the server and then to other clients.
  - with client-authority: the client is authoritative for the entity.
  - with server-authority: after the initial spawn, the server is authoritative for the entity.
    - can use client-prediction...


## Running the example

To start the server, run `cargo run --example client_replication server`

Then you can launch multiple clients with the commands:

- `cargo run --example client_replication client -c 1`

- `cargo run --example client_replication client -c 2 --client-port 2000`
