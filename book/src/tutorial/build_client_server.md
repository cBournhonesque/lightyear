# Setting up the client and server

The client and server will both be bevy Entities to which you can add components to customize their networking behaviour.
Here are some of the common components:
- [`Link`] represents an IO link between a local peer and a remote peer that can be used to send and receive raw bytes
- [`Transport`] adds the capability of setting up various Channels that each provide different reliability/ordering guarantees for a group of bytes
- [`MessageManager`], [`MessageSender<M>`], [`MessageReceiver<M>`] are used to send and receive messages over the network.
  A message is any rust type that can be serialized/deserialize into raw bytes.
- [`ReplicationManager`] and [`ReplicationSender`] can be added to the entity to enable replicating entities and components over the network.

## Client

A client is simply an entity with a [`Link`] to which the [`Client`] marker component is added.
The marker component is used in conjunction with the protocol to customize the behaviour of the link entity.
For example if a message is added to the protocol with
```rust,noplayground
app.add_message::<Message1>()
  .add_direction(NetworkDirection::ServerToClient);
```
then a `MessageReceive<Message1>` component will automatically be added to any `Client` entity.


## Server

Similarly, a server is an entity to which the [`Server`] marker component is added.
The [`Server`] component is a `RelationshipTarget`. Everytime a new io link is established with a remote peer,
a new entity will be spawned with the [`LinkOf`] component that will mark that [`Link`] as being a child of the [`Server`].


Next we will start adding systems to the client and server.