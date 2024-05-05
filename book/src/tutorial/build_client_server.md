# Setting up the client and server

## Client

A client is simply a bevy plugin: [ClientPlugin](https://docs.rs/lightyear/latest/lightyear/client/plugin/struct.ClientPlugin.html)

You create it by providing a [`ClientConfig`](https://docs.rs/lightyear/latest/lightyear/client/config/struct.ClientConfig.html) struct.

You can see how it is defined in the example [here](https://github.com/cBournhonesque/lightyear/blob/main/examples/simple_box/src/main.rs#L175).

### Shared Config

Some parts of the configuration must be shared between the server and the client to work correctly, so we define them in a separate function that can be re-used for both:
```rust
pub fn shared_config(mode: Mode) -> SharedConfig {
    SharedConfig {
        /// How often the client will send packets to the server (by default it is every frame).
        /// Currently, the client only works if it sends packets every frame, for proper input handling.
        client_send_interval: Duration::default(),
        /// How often the server will send packets to clients? You can reduce this to save bandwidth.
        server_send_interval: Duration::from_millis(40),
        /// The tick rate that will be used for the FixedUpdate schedule
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / 64.0),
        },
        /// Here we make the `Mode` an argument so that we can run `lightyear` either in `Separate` mode (distinct client and server apps)
        /// or in `HostServer` mode (the server also acts as a client).
        mode,
    }
}
```

### ClientConfig

The [ClientConfig](https://docs.rs/lightyear/latest/lightyear/client/config/struct.ClientConfig.html) struct lets us configure the client. There are a lot of parameters that can be configured,
but for this demo we will mostly use the defaults.

```rust,noplayground
  let client_config = client::ClientConfig {
      shared: shared_config(Mode::Separate),
      net: net_config,
      ..default()
  };
  let client_plugin = client::ClientPlugin::new(client_config);
```

The [NetConfig](https://docs.rs/lightyear/latest/lightyear/prelude/client/enum.NetConfig.html) doesn't have any Default value and needs to be provided; it defines how (i.e. what transport layer) the client will connect to the server.
There are multiple options available, but for this demo we will use the `Netcode` option.
[netcode](https://github.com/mas-bandwidth/netcode/blob/main/STANDARD.md) is a standard to establish a connection between two hosts, and we can use any io layer (UDP, WebSocket, WebTransport, etc.) to send the actual bytes.

You will need to provide the [IoConfig](https://docs.rs/lightyear/latest/lightyear/transport/io/struct.IoConfig.html) which defines the transport layer (how the raw packets are sent),
with the possibility of using a [LinkConditionerConfig](https://docs.rs/lightyear/latest/lightyear/prelude/struct.LinkConditionerConfig.html) to simulate network conditions.
Here are the different possible transport options: [TransportConfig](https://docs.rs/lightyear/latest/lightyear/transport/io/enum.TransportConfig.html)


```rust,noplayground
/// You can add a link conditioner to simulate network conditions
let link_conditioner = LinkConditionerConfig {
    incoming_latency: Duration::from_millis(100),
    incoming_jitter: Duration::from_millis(0),
    incoming_loss: 0.00,
};
/// Here we use the `UdpSocket` transport layer, with the link conditioner
let io_config = IoConfig::from_transport(TransportConfig::UdpSocket(addr))
    .with_conditioner(link_conditioner);
```

With the `Netcode` option, we use a [ConnectToken](https://docs.rs/lightyear/latest/lightyear/connection/netcode/struct.ConnectToken.html) to secure the connection.
Normally, a third-party server would generate the `ConnectToken` and send it securely to the client.

For this demo, we will use the `Manual` option, which lets us manually build a `ConnectToken` on the client using a private key shared between the client and the server.

```rust,noplayground
let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), server_port);
let auth = Authentication::Manual {
    // server's IP address
    server_addr,
    // ID to uniquely identify the client
    client_id: client_id,
    // private key shared between the client and server
    private_key: KEY,
    // PROTOCOL_ID identifies the version of the protocol
    protocol_id: PROTOCOL_ID,
};
```

Now we can build the complete `NetConfig`:
```rust,noplayground
let net_config = NetConfig::Netcode {
    auth,
    io: io_config,
    ..default()
};
```


## Server

Building the server is very similar to building the client; we need to provide a `ServerConfig` struct.
```rust,noplayground
let server_config = server::ServerConfig {
    shared: shared_config(Mode::Separate),
    net: net_configs,
    ..default()
};
let server_plugin = server::ServerPlugin::new(server_config);
```

The server can listen for client connections using multiple transports at the same time!
You can do this by providing multiple [NetConfig](https://docs.rs/lightyear/latest/lightyear/prelude/server/enum.NetConfig.html) to the server.

The `simple_box` example generates the various `NetConfig`s by parsing the `settings.ron` file, but you can also just
define them manually:

```rust,noplaground
let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.port);
/// You need to provide the private key and protocol id when building the `NetcodeConfig`
let netcode_config = NetcodeConfig::default()
    .with_protocol_id(PROTOCOL_ID)
    .with_key(KEY);
/// You can also add a link conditioner to simulate network conditions for packets received by the server
let link_conditioner = LinkConditionerConfig {
    incoming_latency: Duration::from_millis(100),
    incoming_jitter: Duration::from_millis(0),
    incoming_loss: 0.00,
};
let net_config = NetConfig::Netcode {
    config: netcode_config,
    io: IoConfig::from_transport(TransportConfig::UdpSocket(server_addr))
        .with_conditioner(link_conditioner),
};
let config = ServerConfig {
    shared: shared_config().clone(),
    /// Here we only provide a single net config, but you can provide multiple!
    net: vec![net_config],
    ..default()
};
/// Finally build the server plugin
let server_plugin = server::ServerPlugin::new(server_config);
```

Next we will start adding systems to the client and server.