# Input handling

Lightyear handles inputs for you by:
- buffering the last few inputs on both client and server
- re-using the inputs from past ticks during rollback
- sending client inputs to the server with redundancy


## Client-side

Input handling is currently done in the FixedUpdate schedule.

There are multiple `SystemSets` involved that should run in the following order:
- `BufferInputs`: the user must add their inputs for the given tick in this system set
- `WriteInputEvents`: we get the inputs for the current tick and return them as the bevy event `InputEvent<I>`
  - notably, during rollback we get the inputs for the older rollback tick
- `ClearInputEvents`: we clear the bevy events. 
- `SendInputMessage`: we prepare a message with the last few inputs. For redundancy, we will send the inputs of the last few frames, so that the server
  can still get the correct input for a given tick even if some packets are lost.



## Server-side

Input handling is also done in the FixedUpdate schedule.
These are the relevant `SystemSets`:
- `WriteInputEvents`: we receive the input message from the client, add the inputs into an internal buffer. Then in this 
  SystemSet we retrieve the inputs for the current tick for the given client. The retrieved inputs will be returned as `InputEvent<I>`
- `ClearInputEvents`: we clear the events