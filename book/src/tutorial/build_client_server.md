# Setting up the client and server

## Client

The client is a `bevy` `Resource` that handles the connection to the server:
- the sending and receiving of messages.
- applying replication events to the client's `World`
- handling the inputs from the user.
- running prediction and interpolation
- and more ...

To set up the client, we will need 3 inputs:
- the `Protocol` that we defined in the previous section
- a `ClientConfig` object that defines all modifiable parameters of the client
- an `Authentication` object that defines how the client will authenticate with the server


### Authentication

This crate uses the [netcode.io](https://github.com/networkprotocol/netcode/blob/master/STANDARD.md) protocol to establish a connection, which is 
a simple protocol to create a secure client/server connection over UDP. The general idea is that the server generates a `ConnectToken` that it sends
to the client outside of this crate (for example via HTTPS). The client
then uses this token to connect to the server in a secure manner.

For this demo, we can use `Authentication::Manual`, which lets us manually build a `ConnectToken` using a private key shared
between the client and the server.

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

### ClientConfig

The `ClientConfig` object lets us configure the client. There are a lot of parameters that can be configured,
but for this demo we will mostly use the defaults.

```rust,noplayground
pub fn shared_config() -> SharedConfig {
    SharedConfig {
        // how often will the server send packets to the client (you can use this to reduce bandwidth used)
        server_send_interval: Duration::from_millis(100),
        // configuration for the FixedUpdate schedule
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / 64.0),
        },
        log: LogConfig {
            level: Level::INFO,
            filter: "wgpu=error,wgpu_hal=error,naga=warn,bevy_app=info,bevy_render=warn"
                .to_string(),
        },
        ..Default::default()
    }
}
// You can add a link conditioner to simulate network conditions
let link_conditioner = LinkConditionerConfig {
    incoming_latency: Duration::from_millis(100),
    incoming_jitter: Duration::from_millis(0),
    incoming_loss: 0.00,
};
let config = ClientConfig {
    shared: shared_config().clone(),
    io: IoConfig::from_transport(TransportConfig::UdpSocket(addr))
        .with_conditioner(link_conditioner),
    ..Default::default()
};
```

There are 2 things we will change:
- `IoConfig`: this lets us define how the transport-layer (sending packets will work). The only transport layer supported now is UDP, so we will use that.
  However we can add a `LinkConditionerConfig` to simulate network conditions: adding jitter, latency, packet loss.
- `SharedConfig`: this lets us define parameters that should be the same between the client and the server:
  - `server_send_interval`: how often does the server send packets to the client? (the client needs to know this for interpolation)
  - `tick`: a tick is the fixed-timestep unit of simulation (which is different than the frame-duration). Both the client and server should use the same tick duration.
  - `log`: the log level of the client (TODO: this should not be shared between client and server)

### Creating the client

Now that we have the `Protocol`, `Authentication` and `ClientConfig`, we can create the client:

```rust,noplayground
let plugin_config = lightyear_shared::client::PluginConfig::new(config, MyProtocol::default(), auth);
app.add_plugins(lightyear_shared::client::Plugin::new(plugin_config));
```

This adds the `Client` resource to the `App`.
The `Client` resource lets you:
- send messages to the server on a given channel: `fn send_message<C: Channel, M: Message>(&mut self, message: M)`
- handle inputs (store them in a local buffer and send them to the server): `fn add_input(&mut self, input: P::Input)`

## Server

Building the server is very similar to building the client; this time we just need the protocol and a `ServerConfig`.

```rust,noplayground
let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.port);
let netcode_config = NetcodeConfig::default()
    .with_protocol_id(PROTOCOL_ID)
    .with_key(KEY);
let link_conditioner = LinkConditionerConfig {
    incoming_latency: Duration::from_millis(100),
    incoming_jitter: Duration::from_millis(0),
    incoming_loss: 0.00,
};
let config = ServerConfig {
    shared: shared_config().clone(),
    netcode: netcode_config,
    io: IoConfig::from_transport(TransportConfig::UdpSocket(server_addr))
        .with_conditioner(link_conditioner),
    ping: PingConfig::default(),
};
let plugin_config =
    lightyear_shared::server::PluginConfig::new(config, MyProtocol::default());
```

Next we will start adding systems to the client and server.