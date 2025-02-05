# Changelog


## [Unreleased]

- Simplified the examples
- Added the `visualizer` feature to get a egui dashboard showing a plot of various lightyear metrics
- Removed `DisabledComponent::<C>` in favor of `DisabledComponents` to have more control over
which components are disabled. In particular, it is now possible to express 'disable all components except these'.
- Made the `ServerConnections` resource private. You can now use `commands.disconnect(client_id)` to disconnect a client.
- Enabled replicating events directly!
  - Add an `Event` to the protocol with `register_event`
  - Replicate an event and buffer it in EventWriter with `send_event`
  - Replicate an event and trigger it with `trigger_event`
- Replaced `MessageEvent.context()` with `MessageContext.from()`, which returns the `ClientId` that send the message
- State is correctly cleaned up when the server is stopped (the ControlledBy and Client entities are correctly despawned)
- Parent-Child hierarchy is now synced properly to the Predicted/Interpolated entities
- Fixed how prediction/interpolation interact with authority transfer.
  - For example in the use case where we spawn an entity on Client 1, replicate it to the server, then give the server authority over the entity,
    a Predicted/Interpolated entities will now get spawned correctly on client 1
- Fixed some edge cases related to InterestManagement
- Fixed a bug where ChannelDirection was not respected (a ClientToServer component would still get replicated from the server to the client) 
- Type-erased the receive-message systems so that we only have one `read_messages` system instead of one system per message type



## 0.18.0 - 2024-12-24