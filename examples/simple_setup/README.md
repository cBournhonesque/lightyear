# Simple setup 


The other examples use a common test harness in the `common` folder so that they can run under many different configurations:
- with multiple transports (WebTransport, UDP, etc.)
- in host-server mode, dedicated-server mode, etc.
- with a link conditioner to fake network conditions
- etc.

However it can become confusing to understand how to setup a simple app using Lightyear.

This example will purely showcase how to setup the lightyear plugins.

## Running the example

There are different 'modes' of operation:

- as a dedicated server with `cargo run -- server`

Then you can launch clients with the commands:

- `cargo run -- client -c 1` (`-c 1` overrides the client id, to use client id 1)
- `cargo run -- client -c 2`