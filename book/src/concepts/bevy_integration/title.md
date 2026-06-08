# Bevy integration

Lightyear is built as a set of Bevy plugins and components. You do not hand a socket to a networking loop and wait for callbacks. You add plugins, spawn entities, attach components, and let Bevy schedules do the work.

That is a good fit for games, but it also means ordering matters. Packets are received before your gameplay systems run, inputs are written before fixed simulation, and outgoing messages are flushed after the frame has had a chance to produce changes.

The important habit is to keep networked simulation in `FixedUpdate` unless you have a specific reason not to. That keeps the server, prediction, interpolation, and time synchronization talking about the same ticks.

This section covers the pieces that usually matter when wiring Lightyear into a Bevy app:

- shared plugins, for types and systems that must exist on both client and server
- client setup, including local input and timeline synchronization
- server setup, including per-client link entities
- events and observer patterns used by the connection and message layers
