# Client

A client is an entity with the `Client` marker component, a `Link`, an IO component, a connection component (`NetcodeClient`), and a `ReplicationReceiver`.

The client's jobs, every frame:
- buffer local inputs in `FixedPreUpdate` (`InputSystems::WriteClientInputs`) so they get sent to the server with the right tick
- run predicted movement in `FixedUpdate`, same code as the server
- receive replicated entities/messages in `PreUpdate` (`ReplicationSystems::Receive`) and interpolated snapshots in `Update`
- send everything out in `PostUpdate` (`ReplicationSystems::Send`)

The [time sync](./time_sync.md) page explains how the client keeps its tick aligned with the server's.
