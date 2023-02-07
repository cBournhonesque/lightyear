
# Tenets

* similar to naia, but tightly integrated with Bevy. No need to wade through WorldProxy, etc.
* re-uses bevy's change detection


* WorldReplication
  * can insert a `Replicate` component to an entity for it to start getting replicated. Can remove it so that the replication stops.
  * every component to replicate also derives a `ReplicableComponent` trait
  * when replication stop, we either delete or not the entity on the client.
  * can define if we want to replicate a component via a reliable or unreliable channel.
  * When we replicate a new entity:
    * all incoming component updates/inserts for that entity are buffered while the entity is waiting to be spawned
    * if a 'despawn' message is received, the entity is not spawned at all?
    * when we spawn it, we send an ack back to the server. (or maybe the ack is just part of when we receive the 'spawn' message)
  * Each component will specify how they are serialized, and we will provide a default efficient serializer.
    * can also optionally provide a delta-serializer.
  * We will generate a protocol automatically from:
    * some bits are reserved for specialized message types
    * otherwise we assign bits for each replicable component + each message type
    * Use reflection when possible? or derive_macro to get the protocol
  * Also need a NetComponentId for serializing components? The number of components serialized is fixed so we can
    generate a new NetComponentId starting from 0, for each of new component added to the protocol.

* Extra networking features
  * client prediction
  * server reconciliation
  * snapshot interpolation

* Protocol
  * Only send components that are changed, via bevy's change detection
  * Delta-compression?
  * Efficient wire representation

* TODO:
  * for a given component/message, need to first transform it into something serializable
  (i.e. convert any entities to NetEntity), and then serialize it with Serde
  * simplify/clarify packet logic
    * all entity-packets/component-updates/messages will be PacketType: Data. This should be included automatically in the packet sending code
  * FLATTEN CODE: less indirection and copying interfaces (for example EntityManager)
  * Separate code into
    * Protocol
      * Channels
      * Components
      * Messages
      * Config
    * Channels (general channel code)
    * Entity
      * net_entity conversion, etc.





## Changelog compared to naia 0.14


* Remove EntityHandle everywhere and just keep using directly Bevy's Entity
* Simplify the WorldChannel. Stop keeping track of a host/remote world.
  * update the entity-channels