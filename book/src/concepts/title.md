# Concepts

There are several layers that enable lightyear to act as a games networking library.
Let's list them from the bottom up (closer to the wire):

- IO: how do we send raw bytes over the network between two peers?
  The `Link` component can be added to an entity to interact with the IO layer. Usually you will directly
  add the io component itself (`WebTransportClientIO`, `UdpIO`, `CrossbeamIO`, etc.), which will add the `Link` component.

- Transport: how do provide reliability/ordering guarantees for the bytes we want to send over the `Link`?
The `Transport` component can be added to provide `Channels`. These channels can be used to define the send_frequency,
priority, ordering, reliability characteristics for the bytes you want to send.

- Messages: how do you go from raw bytes to rust types?
The `MessageManager`/`MessageSender`/`MessageReceiver` components will be required to serialize/deserialize from rust types into
raw bytes that you can send over the `Link` or `Transport`. It is also responsible for mapping Entities from the remote World to 
the local World.

- Connection: how do we get a persistent connection on top of a link?
A Link can be ephemeral, for example if it's simply an UDPSocket. Sometimes you want a more long-term identifier for the different
peers that you are linked to. For example so that when a client disconnects and reconnects you can recognize them as the same client
even if their socket port changed.
Currently we have two layers that can give you a persistent connection: Netcode or Steam.

- Replication: how do you replicate components between the remote World and the local World

- advanced replication: prediction, interpolation, etc.
