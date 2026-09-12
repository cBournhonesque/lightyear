# Timeline Switch (carry demo)

Server-authoritative physics demo for client-local prediction switching
(`TimelineSwitch`): far players are **interpolated**, and they get switch to
**predicted** when they get near the local player.
This allows you to collide/interact with other players, while avoiding having to
predict the entire world.

We include other objects that can be carried to test how child entities/colliders
work during timeline switches.

## Controls

`W/A/S/D` move, `Space` jump, `E` drop object.

## Running it

- Server (headless): `cargo run -p timeline_switch -- --headless=true server`
- Client 1: `cargo run -p timeline_switch -- client -c 1`
- Client 2: `cargo run -p timeline_switch -- client -c 2`


### Testing in wasm with webtransport

NOTE: I am using the [bevy cli](https://github.com/TheBevyFlock/bevy_cli) to build and serve the wasm example.

To test the example in wasm, you can run the following commands: `bevy run web`

The repo includes a pre-generated self-signed WebTransport certificate and digest, so you do not need to run the certificate generator for the usual local workflow while that certificate is valid. If it expires, or if you want to replace it, generate a new temporary self-signed certificate with:
- `cargo run -p generate_certificate` (writes `certificates/cert.pem`, `certificates/key.pem`, and `certificates/digest.txt`; rebuild wasm clients after regenerating so they embed the new digest)
