# Input handling

Lightyear handles inputs for you by:
- buffering the last few inputs on both client and server
- re-using the inputs from past ticks during rollback
- sending client inputs to the server with redundancy


## Client-side

Input handling runs across several schedules. The `InputSystems` sets involved, in order:

- `ReceiveInputMessages` (in `PreUpdate`, before rollback): receive input messages from other clients (matters for P2P / predicting remote players)
- `WriteClientInputs` (in `FixedPreUpdate`): **this is where you write**. Put your input-gathering system here; it updates the local `ActionState<I>` for the current tick
- `BufferClientInputs` (in `FixedPreUpdate`, right after): lightyear moves the `ActionState` into the input buffer. During rollback, this set instead loads the historical input back into the `ActionState`, so your simulation re-runs with the right values
- `PrepareInputMessage` / `SendInputMessage` (in `PostUpdate`): pack the last few ticks of inputs into a message (with redundancy, so lost packets don't lose inputs) and send it to the server
- `RestoreInputs` (in `FixedPostUpdate`), `CleanUp` (in `PostUpdate`): housekeeping so buffers don't grow forever



## Server-side

On the server the inputs arrive as messages, get buffered per client, and are then served tick-by-tick: when the server simulates tick T, it hands your systems the inputs the client buffered for tick T. That's the tick-sync guarantee: your input for tick T runs on the server at tick T.

The practical consequence is the same as before: read inputs from the `ActionState<I>` component, and run the simulation that consumes them in the `FixedUpdate` schedule.
