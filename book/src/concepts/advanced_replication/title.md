# Advanced replication

This section is about the tools you reach for after basic server-to-client replication works.

The current model is intentionally server-authoritative:

- replicated entities are sent from server to clients
- visibility is handled through Replicon filters, immediate visibility changes, and rooms
- clients send intent through inputs, messages, or events
- prediction and interpolation are client-side behavior on top of server state

The useful mental model is:

- the server owns the replicated state
- the client receives remote entities from the server
- `Predicted` and `Interpolated` are markers used by local systems
- component histories keep enough authoritative state for rollback or smoothing

Some advanced features are still moving while the Replicon backend settles. When a page describes a limitation, assume the limitation is real for the current release rather than a design recommendation.
