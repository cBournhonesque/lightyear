# Bevy integration

Lightyear is a set of bevy plugins. There are two plugin groups:

- `ClientPlugins { tick_duration }` for apps that act as clients
- `ServerPlugins { tick_duration }` for apps that act as servers

(An app can add both; that's host-server mode. The `simple_box` example does exactly that with `Mode::HostClient`.)

On top of those, your game adds:
- a shared protocol plugin (components, messages, inputs, channels) on both sides, added after `ClientPlugins`/`ServerPlugins`
- client-specific systems (input buffering, predicted movement) — see [client](./client/title.md)
- server-specific systems (spawning players, authoritative movement) — see [server](./server.md)

The pages in this section explain how lightyear hooks into bevy's schedules ([system order](./system_order.md)) and how client [time sync](./client/time_sync.md) works. Messages are covered in the [protocol](../replication/protocol.md) page.
