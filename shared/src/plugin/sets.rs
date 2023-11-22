use bevy::prelude::SystemSet;

/// System sets related to Replication
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ReplicationSet {
    /// System Set to gather all the replication updates to send
    SendEntityUpdates,

    SendComponentUpdates,
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum MainSet {
    /// Systems that receive data (buffer any data received from transport, and read
    /// data from the buffers)
    ///
    /// Runs in `PreUpdate`.
    Receive,
    ReceiveFlush,
    /// Systems that send data (buffer any data to be sent, and send any buffered packets)
    ///
    /// Runs in `PostUpdate`.
    Send,
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum FixedUpdateSet {
    /// System that runs at the very start of the FixedUpdate schedule to increment the ticks
    TickUpdate,
    /// Main loop (with physics, game logic) during FixedUpdate
    Main,
    MainFlush,
}

// To fix that we could do:
// First:
// - Update RealTime/VirtualTime/Time
// PreUpdate:
// - ReceiveIO (receive packets from io)
// - ReadMessages that are not tick-buffered (AND MAYBE THE TICK-BUFFERED ACTUALLY)
// - UpdateEvents that are not tick-buffered (and add them to BevyEvents)
// - CheckIfRollbackIsNeeded
// - Clear predicted histories
// - MaybeApplyRollback
// FixedUpdateLoop:
// - Update Time<Fixed>
// - Run (maybe multiple times) FixedUpdate:
//   - Update Tick -> can run in rollback since the accumulator will be empty
//   - [ SOME SYSTEMS THAT NEED ACCURATE TICK? ]
//      - possibly sync_manager (monitoring lag, etc.) -> maybe not needed
//      - read/send messages that are tick-buffered (Tick-buffered channels). For now only inputs?  -> maybe not needed
//        - read: actually we can read them in PreUpdate, and then they will be put in tick-buffer with correct tick
//        - send: the client doesn't send inputs at a FixedUpdate precision, so it's ok if we send them once per frame. Just send them for all ticks in the frame. ]
//   - Gather input for tick!
//   - Physics (we can pop from the input buffer to get accurately the event we want)
//   - IncrementRollbackTick
//   - UpdatePredictedHistory (add changed predicted components history with accurate tick) -> run in rollback again to repopulate the history!

// Update:
// - rest of game: send messages, etc.
// PostUpdate:
// - SendEntityUpdates: gather all entity actions (spawn, despawn, etc.) and buffer them
// - SendComponentUpdates: gather all entity updates (component_update) and buffer them
// - SendInputs: gather new inputs from the frame; decide how we choose each input per tick; then send inputs. (we send inputs from the last 10 frames) ?
// - Send: send all buffered messages
