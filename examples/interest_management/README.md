# Interest management

A simple example that shows how to use Lightyear to perform interest management.

Interest management is a technique to reduce the amount of data that is sent to each client:
we want to send only the data that is relevant to each client.

In this example, we are going to replicate entities that are within a certain distance of the client.



## Running the example

To start the server, run `cargo run --example interest_management -- server`

Then you can launch multiple clients with the commands:

- `cargo run --example interest_management -- client -c 1`

- `cargo run --example interest_management -- client -c 2 --client-port 2000`


### Testing in wasm

To test the example in wasm, you can run the following commands:
- `sh examples/generate.sh` (to generate the temporary SSL certificates)
- `cargo run --example interest_management --features webtransport -- server --transport web-transport` to start the server
- You will then need to copy the `digest` string for the server certificate and paste it in the `examples/interest_management/client.rs` file.
  Replace the value at the line 
```
let certificate_digest =
String::from("09945594ec0978bb76891fb5de82106d7928191152777c9fc81bec0406055159");
```
- then start the client wasm test with
  `RUSTFLAGS=--cfg=web_sys_unstable_apis wasm-pack test --chrome --example interest_management --features webtransport --target wasm32-unknown-unknown`
