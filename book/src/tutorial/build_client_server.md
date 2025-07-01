# Setting up the client and server

The client and server will both be bevy Entities to which you can add components to customize their networking behaviour.
Here are some of the common components:
- [`Link`] represents an IO link between a local peer and a remote peer that can be used to send and receive raw bytes
- [`Transport`] adds the capability of setting up various Channels that each provide different reliability/ordering guarantees for a group of bytes
- [`MessageManager`], [`MessageSender<M>`], [`MessageReceiver<M>`] are used to send and receive messages over the network.
  A message is any rust type that can be serialized/deserialize into raw bytes.
- [`ReplicationManager`] and [`ReplicationSender`] can be added to the entity to enable replicating entities and components over the network.

## Link

The [`Link`] component is the primary component that represents a connection between two peers. Every network connection is represented by a link. On the server side, you have a [`Server`] 
component which spawns a new entity with a [`Link`] component every time a new client connects to it. The [`LinkOf`] relationship component is added on these entities to help you identify the 
[`Server`] that they are connected to.

The link is agnostic to the actual io layer, you will have to pair it with an actual io component (`UdpIo`, `WebTransportIo`, etc.) to start sending and receiving bytes.

## Connection

Lightyear makes a distinction between a [`Link`] and a `Connection`.
A `Link` is a low-level component that represents a raw IO link, which can be used to send and receive bytes.
A `Connection` is a link that has a long-lived identifiers attached to them. The `LocalId` and `RemoteId` components are used to store the `PeerId` of the local and remote peers, respectively.
The `PeerId` is a unique identifier for a peer in the network, which can be used to identify the peer across multiple connections. (a client could get disconnected and reconnect with a different 
[`Link`],
but still have the same `PeerId`).

The lifecycle of a connection is controlled by several sets of components.

You can trigger [`Connect`] to start the connection, and [`Disconnect`] to stop it.

The [`Disconnected`], [`Connecting`], [`Connected`] components represent the current state of the connection.

On the server, [`Start`] and [`Stop`] components are used to control the server's listening state.
The [`Stopped`], [`Starting`], [`Started`] components represent the current state of the connection.

While a client is disconnected, you can update its configuration (`ReplicationSender`, `MessageManager`, etc.), it will be applied on the next connection attempt.


## Client

A client is simply an entity with a [`Link`] to which the [`Client`] marker component is added.
The marker component is used in conjunction with the protocol to customize the behaviour of the link entity.
For example if a message is added to the protocol with
```rust,noplayground
app.add_message::<Message1>()
  .add_direction(NetworkDirection::ServerToClient);
```
then a `MessageReceive<Message1>` component will automatically be added to any `Client` entity.

You can also just add the [`MessageReceiver<M>`] component directly to the client entity to receive messages of type `M` from the server.

Here is how you can set up a client in your app:

```rust,ignore
let auth = Authentication::Manual {
    server_addr: SERVER_ADDR,
    client_id: 0,
    private_key: Key::default(),
    protocol_id: 0,
};
let client = commands
    .spawn((
        Client::default(),
        LocalAddr(CLIENT_ADDR),
        PeerAddr(SERVER_ADDR),
        Link::new(None),
        ReplicationReceiver::default(),
        NetcodeClient::new(auth, NetcodeConfig::default())?,
        UdpIo::default(),
    ))
    .id();
commands.trigger_targets(Connect, client);
```

Let's walk through this:
- we add the [`Client`] marker component to the entity to identify it as a client.
- we manually specify the [`LocalAddr`] and [`PeerAddr`] components to define the local and remote addresses of the link.
- we add the [`Link`] component to the entity, which will be used to send and receive raw bytes over the network.
- we add the [`ReplicationReceiver`] component to the entity, which will be used to receive replicated entities and components from the server.
- every [`Link`] needs to use a connection layer; either Netcode or Steam. Here we will use Netcode. For testing purposes we will use the `Manual` authentication method, where we have to specify the 
  server address and client ID.
- finally we add the [`UdpIo`] component to the entity, which will be used to send and receive UDP packets over the network.

Finally we trigger the [`Connect`] trigger to start the connection process.


## Server

Similarly, a server is an entity to which the [`Server`] marker component is added.
The [`Server`] component is a `RelationshipTarget`. Everytime a new io link is established with a remote peer,
a new entity will be spawned with the [`LinkOf`] component that will mark that [`Link`] as being a child of the [`Server`].

```rust,ignore
let server = commands
    .spawn((
        NetcodeServer::new(NetcodeConfig::default()),
        LocalAddr(SERVER_ADDR),
        ServerUdpIo::default(),
    ))
    .id();
commands.trigger_targets(Start, server);
```

We need to add `NetcodeServer` because we need a connection layer. This will automatically insert the [`Server`] component.
We also need to specify the [`LocalAddr`] component to define the local address of the server.
The IO layer we choose is UDP, so we add the [`ServerUdpIo`] component to the entity.

Finally we trigger the [`Start`] trigger so that the server can start listening for incoming connections.

Next we will start adding systems to the client and server.