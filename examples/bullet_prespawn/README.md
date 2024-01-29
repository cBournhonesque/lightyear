# Features


This example showcases how prespawning player objects on the client side works:
- you just have to add a `PreSpawnedPlayedObject` component to the pre-spawned entity. The system that spawns the entity can be identical in the client and the server
- the client spawns the entity immediately in the predicted timeline
- when the client receives the server entity, it will match it with the existing pre-spawned entity!



https://github.com/cBournhonesque/lightyear/assets/8112632/ee547c32-1f14-4bdc-9e6d-67f900af84d0



# Usage

- Run the server with: `cargo run -- server --headless`
- Run the clients with:
`cargo run -- client -c 1`
`cargo run -- client -c 2`



### Running in wasm

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
