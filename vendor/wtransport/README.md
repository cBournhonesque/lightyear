<p align="center">
  <img src="https://raw.githubusercontent.com/BiagioFesta/wtransport/master/imgs/logo.svg" alt="WTransport Logo" />
</p>

[![Documentation](https://docs.rs/wtransport/badge.svg)](https://docs.rs/wtransport/)
[![Crates.io](https://img.shields.io/crates/v/wtransport.svg)](https://crates.io/crates/wtransport)
[![CI](https://github.com/BiagioFesta/wtransport/actions/workflows/ci.yml/badge.svg)](https://github.com/BiagioFesta/wtransport/actions/workflows/ci.yml)
[![Zulip chat](https://img.shields.io/badge/zulip-join_chat-brightgreen.svg)](https://wtransport.zulipchat.com/)

# WTransport
[WebTransport](https://datatracker.ietf.org/doc/html/draft-ietf-webtrans-http3/) protocol, pure-*rust*, *async*-friendly.

## Introduction

WebTransport is a new protocol being developed to enable *low-latency*, *bidirectional* communication between clients and servers over the web.
It aims to address the limitations of existing protocols like *HTTP* and *WebSocket* by offering a more *efficient* and *flexible* transport layer.

### Benefits of WebTransport
* :rocket: **Low latency**: WebTransport is designed to minimize latency, making it suitable for real-time applications such as gaming, video streaming, and collaborative editing.
* :arrows_counterclockwise: **Bidirectional communication**: WebTransport allows simultaneous data exchange between the client and server, enabling efficient back-and-forth communication without the need for multiple requests.
* :twisted_rightwards_arrows: **Multiplexing**: With WebTransport, multiple streams can be multiplexed over a single connection, reducing overhead and improving performance.
* :lock: **Security**: WebTransport benefits from the security features provided by the web platform, including transport encryption and same-origin policy.

 <p align="center">
   <a href="https://docs.rs/wtransport/latest/wtransport/">Check Library Documentation</a>
 </p>

### Notes
Please be aware that WebTransport is still a *draft* and not yet standardized.
The *WTransport* library, while functional, is not considered completely production-ready.
It should be used with caution and may undergo changes as the WebTransport specification evolves.

## Simple API
<table>
<tr>
<th> Server </th>
<th> Client </th>
</tr>
<tr>
<td>

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let config = ServerConfig::builder()
        .with_bind_default(4433)
        .with_certificate(certificate)
        .build();

    let connection = Endpoint::server(config)?
        .accept()
        .await     // Awaits connection
        .await?    // Awaits session request
        .accept()  // Accepts request
        .await?;   // Awaits ready session

    let stream = connection.accept_bi().await?;
    // ...
}
```

</td>
<td>

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let config = ClientConfig::default();

    let connection = Endpoint::client(config)?
        .connect("https://[::1]:4433")
        .await?;

    let stream = connection.open_bi().await?.await?;
    // ...
}
```

</td>
</tr>
</table>

## Getting Started
### Clone the Repository
```bash
git clone https://github.com/BiagioFesta/wtransport.git
```
```bash
cd wtransport/
```

### Run `Full` Example

The [`examples/full.rs`](wtransport/examples/full.rs) is a minimal but complete server example that demonstrates the usage of WebTransport.

You can run this example using *Cargo*, Rust's package manager, with the following command:
```bash
cargo run --example full
```

This example initiates an *echo* WebTransport server that can receive messages. It also includes an integrated HTTP server and
launches Google Chrome with the necessary options to establish connections using self-signed TLS certificates.

## Examples
* [Local Examples](wtransport/examples/)

## Other languages

WTransport has bindings for the following languages:

- Elixir: [wtransport-elixir](https://github.com/bugnano/wtransport-elixir)
