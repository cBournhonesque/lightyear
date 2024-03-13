# Examples

This folder contains various examples that showcase various `lightyear` features.

Each example runs in a similar way, unless specified:
- start the server: `cargo run -- server --headless`
- start the client: `cargo run -- client -c 1`

You can use the CLI to specify various options (client_id, transport, etc.)



### Note for WebTransport

By the default the transport that is used is `WebTransport`. Note that this comes with a limitation: webtransport requires that a certificate is provided to authenticate the connection.
Self-issued certificates have a maximum duration of 2 weeks only!

If you need to generate a new certificate, you can run (from the root of the repository):
- `sh examples/certificates/generate.sh`
- then run the server, which will print the digest of the certificate being used (something like: 
```
Generated self-signed certificate with digest: 2b:08:3b:2a:2b:9a:ad:dc:ed:ba:80:43:c3:1a:43:3e:2c:06:11:a0:61:25:4b:fb:ca:32:0e:5d:85:5d:a7:56
```)
- then in the client, you need to specify the certificate digest to use for webtransport (for example [here](https://github.com/cBournhonesque/lightyear/blob/main/examples/simple_box/src/client.rs#L34))
