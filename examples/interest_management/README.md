# Interest management

A simple example that shows how to use Lightyear to perform interest management.

Interest management is a technique to reduce the amount of data that is sent to each client:
we want to send only the data that is relevant to each client.

In this example, we are going to replicate entities that are within a certain distance of the client.

https://github.com/cBournhonesque/lightyear/assets/8112632/41a6d102-77a1-4a44-8974-1d208b4ef798

## Running the example

To start the server, run `cargo run  -- server -t udp`

Then you can launch multiple clients with the commands:

- `cargo run  -- client -c 1 -t udp`
- `cargo run  -- client -c 2 --client-port 2000 -t udp`

### Testing webtransport

- `cargo run  -- server`
- `cargo run  -- client -c 1`


### Testing webtransport in wasm

https://github.com/cBournhonesque/lightyear/assets/8112632/4ee0685b-0ac6-42c8-849a-28896a158508

NOTE: I am using [trunk](https://trunkrs.dev/) to build and serve the wasm example.

To test the example in wasm, you can run the following commands:
- `sh examples/generate.sh` (to generate the temporary SSL certificates, they are only valid for 2 weeks)
- `cargo run -- server` to start the server
- You will then need to copy the certificate digest string that is outputted by the server in the logs and paste it in the `examples/interest_management/client.rs` file.
  Replace the certificate value like so:
```
let certificate_digest =
String::from("09945594ec0978bb76891fb5de82106d7928191152777c9fc81bec0406055159");
```
- then start the client wasm test with `trunk serve`

NOTE:
- the wasm example seems to work better in release mode!
