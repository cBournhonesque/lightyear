//! The InputBuffer contains a history of the ActionState for each tick.
//!
//! It is used for several purposes:
//! - the client's inputs for tick T must arrive before the server processes tick T, so they are stored
//!   in the buffer until the server processes them. The InputBuffer can be updated efficiently by receiving
//!   a list of `ActionDiff`s compared from an initial `ActionState`
//! - to implement input-delay, we want a button press at tick t to be processed at tick t + delay on the client.
//!   Therefore, we will store the computed ActionState at tick t + delay, but then we load the ActionState at tick t
//!   from the buffer
use super::input_message::InputSnapshot;
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use bevy_ecs::component::Component;
use bevy_reflect::Reflect;
use bevy_utils::prelude::DebugName;
use core::fmt::{Debug, Formatter};
use core::time::Duration;
use lightyear_core::tick::Tick;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{error, info, trace};

/// Maximum number of ticks retained in an [`InputBuffer`].
///
/// The buffer is a fixed ring: once the window would exceed this, writes evict
/// the oldest ticks, so every write path is structurally bounded (no unbounded
/// growth from far-future ticks). This must stay above worst-case legitimate
/// retention (`max_rollback_ticks + input delay + redundancy`, ≈ 20 + few + 25
/// by default); only degenerate traffic ever hits the cap.
pub const INPUT_BUFFER_CAPACITY: usize = 64;

/// Buffer that stores a value (usually Inputs) for the last few ticks.
///
/// S is the type of the InputSnapshot.
/// M is present in case the InputSnapshot does not have a generic.
#[derive(Component, Reflect)]
pub struct InputBuffer<S, M> {
    pub start_tick: Option<Tick>,
    /// Fixed ring holding the window `[start_tick, start_tick + len)`, oldest first.
    /// The slot for `tick` is `slots[(head + (tick - start_tick)) % INPUT_BUFFER_CAPACITY]`.
    /// Slots hold materialized snapshots (`None` = explicit neutral): wire
    /// compression (`SameAsPrecedent`) is resolved on write, so reads are O(1)
    /// indexing with no chain walks. Bounded: writes past capacity evict the
    /// oldest ticks instead of growing.
    slots: [Option<S>; INPUT_BUFFER_CAPACITY],
    /// Ring index of `start_tick`. Only meaningful when `len > 0`.
    head: usize,
    /// Number of live slots. `end_tick = start_tick + len - 1`.
    len: usize,
    /// For remote inputs, keep track of the last tick we have received from the remote.
    /// (this is necessary because even without receiving a remote tick we keep updating the buffer with
    /// predicted inputs)
    pub last_remote_tick: Option<Tick>,
    #[reflect(ignore)]
    pub marker: core::marker::PhantomData<M>,
}

impl<S: Debug, M> Debug for InputBuffer<S, M> {
    #[inline]
    fn fmt(&self, f: &mut Formatter) -> ::core::fmt::Result {
        f.debug_struct("InputBuffer")
            .field("start_tick", &self.start_tick)
            .field("len", &self.len)
            .field("last_remote_tick", &self.last_remote_tick)
            .finish()
    }
}

impl<T: Debug, M> core::fmt::Display for InputBuffer<T, M> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        let ty = DebugName::type_name::<T>();

        let Some(tick) = self.start_tick else {
            return write!(f, "EmptyInputBuffer");
        };

        let buffer_str = (0..self.len)
            .map(|i| {
                let item = &self.slots[(self.head + i) % INPUT_BUFFER_CAPACITY];
                let str = match item {
                    None => "Absent".to_string(),
                    Some(data) => format!("{data:?}"),
                };
                format!("{:?}: {}\n", tick + i as i32, str)
            })
            .collect::<Vec<String>>()
            .join("");
        write!(f, "InputBuffer<{ty:?}>:\n {buffer_str}")
    }
}

/// We use this structure to efficiently compress the inputs that we send to the server
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Reflect)]
pub enum Compressed<T> {
    // TODO: maybe we don't need Absent? because if the Input is missing we just predict that it was the SameAsPrecedent (with some decay)
    Absent,
    SameAsPrecedent,
    Input(T),
}

impl<T> From<Option<T>> for Compressed<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Compressed::Input(value),
            _ => Compressed::Absent,
        }
    }
}

impl<T, M> Default for InputBuffer<T, M> {
    fn default() -> Self {
        Self {
            slots: core::array::from_fn(|_| None),
            head: 0,
            len: 0,
            start_tick: None,
            last_remote_tick: None,
            marker: Default::default(),
        }
    }
}

/// What to do when a tick has no confirmed input in the buffer.
///
/// This is the per-caller half of [`InputBuffer::resolve`]: the buffer always
/// resolves confirmed ticks exactly, and the caller picks one of these for
/// the miss case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissPolicy {
    /// Leave the live state alone.
    Keep,
    /// Reset the live state to the neutral snapshot.
    Neutral,
    /// Recompute a prediction from the last confirmed input (buffer untouched).
    Predict {
        /// Log an error on a miss: lockstep must never predict.
        lockstep: bool,
    },
}

/// How a buffer read resolves for one tick. See [`InputBuffer::resolve`].
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution<'a, T> {
    /// Confirmed input for the tick.
    Exact(&'a T),
    /// Recomputed prediction; the buffer was not modified.
    Predicted(T),
    /// No input: the caller should apply the neutral snapshot.
    Neutral,
    /// No input: the caller should leave the live state alone.
    Untouched,
}

impl<T> Resolution<'_, T> {
    /// The snapshot to write into the live state, if the resolution carries one.
    /// (`Neutral` intentionally returns `None`: the caller supplies the default.)
    pub fn snapshot(&self) -> Option<&T> {
        match self {
            Resolution::Exact(snapshot) => Some(snapshot),
            Resolution::Predicted(snapshot) => Some(snapshot),
            Resolution::Neutral | Resolution::Untouched => None,
        }
    }
}

impl<T: Clone + PartialEq, M> InputBuffer<T, M> {
    /// Ring index for `tick`. `None` when `tick` is outside `[start_tick, end_tick]`.
    fn index(&self, tick: Tick) -> Option<usize> {
        let start_tick = self.start_tick?;
        // `Tick - Tick` is an `i32`, negative when `tick < start_tick`.
        let offset = tick - start_tick;
        if offset < 0 || self.len == 0 {
            return None;
        }
        let offset = offset as usize;
        if offset >= self.len {
            return None;
        }
        Some((self.head + offset) % INPUT_BUFFER_CAPACITY)
    }

    /// Append a slot at the back, evicting the oldest tick once the window is full.
    ///
    /// Eviction only triggers on degenerate windows (see [`INPUT_BUFFER_CAPACITY`]);
    /// the retention pops keep legitimate windows far smaller. Slots are
    /// materialized, so eviction drops independent values — no repair needed.
    fn push_back(&mut self, value: Option<T>) {
        if self.len == INPUT_BUFFER_CAPACITY {
            self.head = (self.head + 1) % INPUT_BUFFER_CAPACITY;
            if let Some(start) = self.start_tick {
                self.start_tick = Some(start + 1);
            }
            let tail = (self.head + self.len - 1) % INPUT_BUFFER_CAPACITY;
            self.slots[tail] = value;
        } else {
            let tail = (self.head + self.len) % INPUT_BUFFER_CAPACITY;
            self.slots[tail] = value;
            self.len += 1;
        }
    }

    /// Number of elements in the buffer
    pub fn len(&self) -> usize {
        self.len
    }

    /// Remove all elements in the buffer that are strictly after `tick`
    /// (leaves `tick` in the buffer)
    pub fn clip_after(&mut self, tick: Tick) {
        let Some(end_tick) = self.end_tick() else {
            return;
        };
        let start_tick = self.start_tick.unwrap();
        if tick < start_tick {
            self.start_tick = None;
            self.len = 0;
            return;
        }
        if tick >= end_tick {
            return;
        }
        self.len = (tick - start_tick + 1) as usize;
    }

    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    /// Set the ActionState for the given tick in the InputBuffer.
    ///
    /// This should be called every tick. The value is stored as-is; wire
    /// compression is re-derived at message-build time, not here.
    pub fn set(&mut self, tick: Tick, value: T) {
        self.set_raw(tick, Compressed::Input(value));
    }

    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    /// Set the ActionState for the given tick in the InputBuffer
    ///
    /// This should be called every tick.
    pub fn set_empty(&mut self, tick: Tick) {
        self.set_raw(tick, Compressed::Absent);
    }

    /// Write one tick from the wire, resolving compression against the buffer.
    ///
    /// `SameAsPrecedent` repeats the previous tick's stored value (`None` when
    /// there is none); gaps between the old end and `tick` repeat the last
    /// stored value the same way. Reads therefore never walk chains.
    pub fn set_raw(&mut self, tick: Tick, value: Compressed<T>) {
        // Resolve first: only committed slots are read, never the slot being written.
        let resolved = match value {
            Compressed::Input(value) => Some(value),
            Compressed::Absent => None,
            Compressed::SameAsPrecedent => self.get(tick - 1u32).cloned(),
        };
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.head = 0;
            self.len = 0;
            self.push_back(resolved);
            return;
        };

        // cannot set lower values than start_tick
        if tick < start_tick {
            return;
        }

        let end_tick = start_tick + (self.len as i32 - 1);

        // NOTE: we fill the value for the given tick, and we repeat the last
        // stored value over the ticks between the old end and `tick`
        // (i.e. if there are any gaps, we consider that the user repeated
        // their last action)
        if tick > end_tick {
            // TODO: Think about how to fill the buffer between ticks
            //  - we want: if an input is missing, we consider that the user did the same action (RocketLeague or Overwatch GDC)

            // TODO: think about whether this is correct or not, it is correct if we always call set()
            //  with monotonically increasing ticks, which I think is the case
            //  maybe that's not correct because the timing information should be different? (i.e. I should tick the action-states myself and set them)
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            let fill = self.get(end_tick).cloned();
            let mut t = end_tick + 1;
            while t < tick {
                trace!("fill ticks");
                self.push_back(fill.clone());
                t = t + 1;
            }
            // add a new value to the buffer, which we will override below
            self.push_back(None);
        }

        // safety: the tick is in the window (`push_back` eviction keeps the newest ticks)
        let index = self.index(tick).unwrap();
        self.slots[index] = resolved;
    }

    /// Like [`pop`](Self::pop), but preserves the most recent entry so it
    /// remains available as a [`predict`](Self::predict) fallback.
    pub fn pop_keeping_last(&mut self, tick: Tick) -> Option<T> {
        let Some((last_tick, _)) = self.get_last_with_tick() else {
            return self.pop(tick);
        };
        let clamped = if tick >= last_tick {
            last_tick - 1
        } else {
            tick
        };
        self.pop(clamped)
    }

    /// Remove all the inputs that are older or equal than the given tick, then return the input
    /// for the given tick
    pub fn pop(&mut self, tick: Tick) -> Option<T> {
        let start_tick = self.start_tick?;
        if tick < start_tick {
            return None;
        }
        if tick > start_tick + (self.len as i32 - 1) {
            // pop everything
            self.len = 0;
            self.start_tick = Some(tick + 1);
            return None;
        }

        // Slots are materialized, so popping just drops independent values and
        // returns the last one. No chain repair needed.
        let mut popped = None;
        for _ in 0..(tick + 1 - start_tick) {
            // front is the oldest value
            popped = self.slots[self.head].take();
            self.head = (self.head + 1) % INPUT_BUFFER_CAPACITY;
            self.len -= 1;
        }
        self.start_tick = Some(tick + 1);
        popped
    }

    /// Get the `ActionState` for the given tick. This does not apply prediction:
    /// - if the tick is outside the range of the buffer, it returns None
    pub fn get(&self, tick: Tick) -> Option<&T> {
        self.index(tick)
            .and_then(|index| self.slots[index].as_ref())
    }

    /// Predict the input for `tick` without storing anything.
    ///
    /// Ticks inside the window resolve exactly; ticks beyond it decay the newest
    /// stored (hence confirmed) input. Predictions are recomputed on every read
    /// from the confirmed anchor, so they can never go stale — unlike stored
    /// predictions, there is nothing to invalidate when new inputs arrive.
    pub fn predict(&self, tick: Tick, tick_duration: Duration) -> Option<T>
    where
        T: InputSnapshot,
    {
        let (last_tick, last) = self.get_last_with_tick()?;
        if tick <= last_tick {
            return self.get(tick).cloned();
        }
        let mut predicted = last.clone();
        for _ in 0..(tick - last_tick) {
            predicted.decay_tick(tick_duration);
        }
        Some(predicted)
    }

    /// Resolve what the live action state should become for `tick`.
    ///
    /// This is the single decision point shared by the client, server, and
    /// render-restore paths: confirmed ticks always resolve exactly, and the
    /// miss policy says what happens otherwise. Predictions are recomputed
    /// from the confirmed anchor and never stored.
    pub fn resolve(
        &self,
        tick: Tick,
        tick_duration: Duration,
        miss: MissPolicy,
    ) -> Resolution<'_, T>
    where
        T: InputSnapshot,
    {
        if let Some(snapshot) = self.get(tick) {
            return Resolution::Exact(snapshot);
        }
        match miss {
            MissPolicy::Keep => Resolution::Untouched,
            MissPolicy::Neutral => Resolution::Neutral,
            MissPolicy::Predict { lockstep } => {
                if lockstep {
                    error!(
                        "We are in lockstep mode but didn't receive an input for tick {tick:?}!"
                    );
                }
                match self.predict(tick, tick_duration) {
                    Some(predicted) => Resolution::Predicted(predicted),
                    None => Resolution::Untouched,
                }
            }
        }
    }

    /// Get latest ActionState present in the buffer
    pub fn get_last(&self) -> Option<&T> {
        let start_tick = self.start_tick?;
        if self.len == 0 {
            return None;
        }
        self.get(start_tick + (self.len as i32 - 1))
    }

    /// Get latest ActionState present in the buffer, along with the associated Tick
    pub fn get_last_with_tick(&self) -> Option<(Tick, &T)> {
        let start_tick = self.start_tick?;
        if self.len == 0 {
            return None;
        }
        let end_tick = start_tick + (self.len as i32 - 1);
        self.get(end_tick)
            .map(|action_state| (end_tick, action_state))
    }

    /// Get the last tick in the buffer
    #[inline(always)]
    pub fn end_tick(&self) -> Option<Tick> {
        self.start_tick
            .map(|start_tick| start_tick + (self.len as i32 - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No-op decay so `predict` on `i32` buffers is deterministic.
    impl InputSnapshot for i32 {
        fn decay_tick(&mut self, _tick_duration: Duration) {}
    }

    #[test]
    fn test_get_set_pop() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();

        input_buffer.set(Tick(4), 0);
        input_buffer.set(Tick(6), 1);
        input_buffer.set(Tick(7), 1);
        input_buffer.set(Tick(8), 1);

        assert_eq!(input_buffer.get(Tick(4)), Some(&0));
        // missing ticks repeat the last stored value
        assert_eq!(input_buffer.get(Tick(5)), Some(&0));
        assert_eq!(input_buffer.get(Tick(6)), Some(&1));
        // every slot holds a materialized value
        assert_eq!(input_buffer.get(Tick(7)), Some(&1));
        assert_eq!(input_buffer.get(Tick(8)), Some(&1));
        // we get None if we try to get a value outside the buffer
        assert_eq!(input_buffer.get(Tick(9)), None);

        assert_eq!(input_buffer.pop(Tick(5)), Some(0));
        assert_eq!(input_buffer.start_tick, Some(Tick(6)));

        assert_eq!(input_buffer.pop(Tick(7)), Some(1));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.get(Tick(8)), Some(&1));
        assert_eq!(input_buffer.len(), 1);
    }

    #[test]
    fn test_set_empty_and_get() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        input_buffer.set_empty(Tick(3));
        assert_eq!(input_buffer.get(Tick(3)), None);
    }

    #[test]
    fn test_set_raw_and_get() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        input_buffer.set_raw(Tick(2), Compressed::Input(7));
        assert_eq!(input_buffer.get(Tick(2)), Some(&7));
        input_buffer.set_raw(Tick(3), Compressed::SameAsPrecedent);
        assert_eq!(input_buffer.get(Tick(3)), Some(&7));
    }

    #[test]
    fn test_get_last_and_get_last_with_tick() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        assert_eq!(input_buffer.get_last(), None);
        assert_eq!(input_buffer.get_last_with_tick(), None);

        input_buffer.set(Tick(1), 10);
        input_buffer.set(Tick(2), 20);
        assert_eq!(input_buffer.get_last(), Some(&20));
        assert_eq!(input_buffer.get_last_with_tick(), Some((Tick(2), &20)));
    }

    #[test]
    fn test_end_tick() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        assert_eq!(input_buffer.end_tick(), None);
        input_buffer.set(Tick(5), 1);
        assert_eq!(input_buffer.end_tick(), Some(Tick(5)));
        input_buffer.set(Tick(7), 2);
        assert_eq!(input_buffer.end_tick(), Some(Tick(7)));
    }

    #[test]
    fn test_pop_with_absent() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        input_buffer.set(Tick(1), 1);
        input_buffer.set(Tick(2), 2);
        input_buffer.set_empty(Tick(3));
        input_buffer.set(Tick(4), 2);
        // Pop up to tick 2
        assert_eq!(input_buffer.pop(Tick(2)), Some(2));
        // Now tick 3 is Absent, so pop returns None
        assert_eq!(input_buffer.pop(Tick(3)), None);
        // Now tick 4 is Input(2)
        assert_eq!(input_buffer.pop(Tick(4)), Some(2));
    }

    #[test]
    fn test_pop_out_of_range() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        input_buffer.set(Tick(10), 5);
        // Pop before start_tick
        assert_eq!(input_buffer.pop(Tick(5)), None);
        // Pop after end_tick
        assert_eq!(input_buffer.pop(Tick(20)), None);
        assert_eq!(input_buffer.len(), 0);
        assert_eq!(input_buffer.start_tick, Some(Tick(21)));
    }

    #[test]
    fn test_server_pop_pattern_preserves_predict_fallback() {
        let mut buf: InputBuffer<i32, i32> = InputBuffer::default();
        buf.set(Tick(10), 42);

        assert_eq!(buf.predict(Tick(13), Duration::default()), Some(42));

        // Mirror server.rs::update_action_state: advance the buffer floor
        // past the last entry. Plain `pop` would wipe the fallback here.
        buf.pop_keeping_last(Tick(11));

        assert_eq!(
            buf.predict(Tick(13), Duration::default()),
            Some(42),
            "advancing the floor past the last entry must preserve the fallback",
        );
    }

    #[test]
    fn test_pop_same_absent_in_gap() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        input_buffer.set(Tick(9), 5);
        input_buffer.set(Tick(10), 5);
        input_buffer.set_empty(Tick(11));
        input_buffer.set_empty(Tick(12));
        input_buffer.set_empty(Tick(13));
        // Pop before start_tick
        assert_eq!(input_buffer.pop(Tick(12)), None);
        assert_eq!(input_buffer.get(Tick(13)), None);
        assert_eq!(input_buffer.len(), 1);
    }

    #[test]
    fn test_len() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        assert_eq!(input_buffer.len(), 0);
        input_buffer.set(Tick(1), 1);
        assert_eq!(input_buffer.len(), 1);
        input_buffer.set(Tick(2), 2);
        assert_eq!(input_buffer.len(), 2);
    }

    #[test]
    fn test_clip_after() {
        let mut input_buffer: InputBuffer<i32, i32> = InputBuffer::default();
        input_buffer.set(Tick(1), 1);
        input_buffer.set(Tick(2), 2);
        input_buffer.set(Tick(3), 2);

        // clip anything strictly after 3: nothing happens
        input_buffer.clip_after(Tick(3));
        assert_eq!(input_buffer.len(), 3);

        input_buffer.clip_after(Tick(1));
        assert_eq!(input_buffer.len(), 1);
        assert_eq!(input_buffer.get(Tick(1)), Some(&1));
        assert_eq!(input_buffer.get(Tick(2)), None);
    }

    /// Verify that `get` returns None for ticks past the buffer end,
    /// while `predict` falls back to the last known input.
    ///
    /// This matters on the server: when the server tick advances past the
    /// last received input, `get` silently drops the input (returns None)
    /// while `predict` returns the most recent value.
    #[test]
    fn test_get_vs_predict_past_buffer_end() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();
        input_buffer.set(Tick(10), 42);
        input_buffer.set(Tick(12), 99);

        // Within range: both return the same value
        assert_eq!(input_buffer.get(Tick(10)), Some(&42));
        assert_eq!(
            input_buffer.predict(Tick(10), Duration::default()),
            Some(42)
        );
        assert_eq!(input_buffer.get(Tick(12)), Some(&99));
        assert_eq!(
            input_buffer.predict(Tick(12), Duration::default()),
            Some(99)
        );

        // Past buffer end: get returns None, predict returns last known
        assert_eq!(input_buffer.get(Tick(15)), None);
        assert_eq!(
            input_buffer.predict(Tick(15), Duration::default()),
            Some(99),
            "predict should fall back to the last known input"
        );

        // Before buffer start: both return None
        assert_eq!(input_buffer.get(Tick(5)), None);
        assert_eq!(input_buffer.predict(Tick(5), Duration::default()), None);
    }

    /// `resolve` always returns confirmed ticks exactly; the miss policy only
    /// governs unknown ticks. (Uses no-op-decay `i32` snapshots, so a prediction
    /// past the end repeats the anchor.)
    #[test]
    fn test_resolve_policy_matrix() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();
        input_buffer.set(Tick(10), 42);
        input_buffer.set(Tick(12), 99);

        // Confirmed ticks resolve exactly under every policy (11 was gap-filled
        // with 42 at write time).
        for miss in [
            MissPolicy::Keep,
            MissPolicy::Neutral,
            MissPolicy::Predict { lockstep: false },
        ] {
            assert_eq!(
                input_buffer.resolve(Tick(10), Duration::default(), miss),
                Resolution::Exact(&42)
            );
            assert_eq!(
                input_buffer.resolve(Tick(11), Duration::default(), miss),
                Resolution::Exact(&42)
            );
            assert_eq!(
                input_buffer.resolve(Tick(12), Duration::default(), miss),
                Resolution::Exact(&99)
            );
        }

        // Unknown ticks follow the miss policy ...
        // ... past the end: prediction repeats the anchor ...
        assert_eq!(
            input_buffer.resolve(
                Tick(15),
                Duration::default(),
                MissPolicy::Predict { lockstep: false }
            ),
            Resolution::Predicted(99)
        );
        // ... lockstep still predicts (after logging); the policy only adds the error.
        assert_eq!(
            input_buffer.resolve(
                Tick(15),
                Duration::default(),
                MissPolicy::Predict { lockstep: true }
            ),
            Resolution::Predicted(99)
        );
        // ... Neutral asks the caller to apply the default ...
        assert_eq!(
            input_buffer.resolve(Tick(15), Duration::default(), MissPolicy::Neutral),
            Resolution::Neutral
        );
        // ... Keep leaves the live state alone ...
        assert_eq!(
            input_buffer.resolve(Tick(15), Duration::default(), MissPolicy::Keep),
            Resolution::Untouched
        );
        // ... and with no confirmed anchor at all, even prediction is Untouched.
        assert_eq!(
            input_buffer.resolve(Tick(5), Duration::default(), MissPolicy::Neutral),
            Resolution::Neutral
        );
        assert_eq!(
            input_buffer.resolve(
                Tick(5),
                Duration::default(),
                MissPolicy::Predict { lockstep: false }
            ),
            Resolution::Untouched
        );

        // Resolving never mutates the buffer.
        assert_eq!(input_buffer.len(), 3);
        assert_eq!(input_buffer.start_tick, Some(Tick(10)));
        assert_eq!(input_buffer.end_tick(), Some(Tick(12)));
    }

    /// Snapshot whose decay visibly accumulates, pinning multi-tick prediction.
    #[derive(Clone, PartialEq, Debug, Default)]
    struct CountingSnapshot(u32);
    impl InputSnapshot for CountingSnapshot {
        fn decay_tick(&mut self, _tick_duration: Duration) {
            self.0 += 1;
        }
    }

    #[test]
    fn test_predict_applies_decay_per_tick() {
        let mut input_buffer = InputBuffer::<CountingSnapshot, i32>::default();
        input_buffer.set(Tick(10), CountingSnapshot(0));
        // in-window ticks resolve exactly, no decay applied
        assert_eq!(
            input_buffer.predict(Tick(10), Duration::default()),
            Some(CountingSnapshot(0))
        );
        // beyond the window: decay applied once per tick past the anchor
        assert_eq!(
            input_buffer.predict(Tick(11), Duration::default()),
            Some(CountingSnapshot(1))
        );
        assert_eq!(
            input_buffer.predict(Tick(13), Duration::default()),
            Some(CountingSnapshot(3))
        );
        // prediction leaves the buffer untouched
        assert_eq!(input_buffer.len(), 1);
        assert_eq!(input_buffer.end_tick(), Some(Tick(10)));
    }

    /// Long runs of identical inputs store every tick; all resolve to the held value.
    #[test]
    fn test_long_hold_chain() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();
        for t in 0..32u32 {
            input_buffer.set(Tick(t), 7);
        }
        assert_eq!(input_buffer.len(), 32);
        for t in 0..32u32 {
            assert_eq!(input_buffer.get(Tick(t)), Some(&7));
        }
        assert_eq!(input_buffer.get_last(), Some(&7));
        assert_eq!(
            input_buffer.predict(Tick(150), Duration::default()),
            Some(7)
        );
    }

    /// The ring caps the window at `INPUT_BUFFER_CAPACITY`, evicting the oldest
    /// ticks. Every retained slot holds its own value, so eviction needs no repair.
    #[test]
    fn test_ring_eviction_drops_oldest() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();
        for t in 0..100u32 {
            input_buffer.set(Tick(t), 7);
        }
        assert_eq!(input_buffer.len(), INPUT_BUFFER_CAPACITY);
        assert_eq!(input_buffer.start_tick, Some(Tick(36)));
        assert_eq!(input_buffer.end_tick(), Some(Tick(99)));
        for t in 36..100u32 {
            assert_eq!(input_buffer.get(Tick(t)), Some(&7));
        }
        assert_eq!(input_buffer.get(Tick(35)), None);
        assert_eq!(input_buffer.get_last(), Some(&7));
    }

    /// `SameAsPrecedent` written behind `Absent` resolves to None at write time —
    /// neutral wins over repeat, including for `get_last`.
    #[test]
    fn test_same_as_precedent_behind_absent_resolves_none() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();
        input_buffer.set(Tick(10), 5);
        input_buffer.set_empty(Tick(11));
        input_buffer.set_raw(Tick(12), Compressed::SameAsPrecedent);
        assert_eq!(input_buffer.get(Tick(12)), None);
        assert_eq!(input_buffer.predict(Tick(12), Duration::default()), None);
        assert_eq!(input_buffer.get_last(), None);
    }

    /// Characterization: `clip_after` below start clears the buffer;
    /// at/above end is a noop.
    #[test]
    fn test_clip_after_edges() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();
        input_buffer.set(Tick(10), 1);
        input_buffer.set(Tick(11), 2);
        input_buffer.clip_after(Tick(20));
        assert_eq!(input_buffer.len(), 2);
        input_buffer.clip_after(Tick(5));
        assert_eq!(input_buffer.len(), 0);
        assert_eq!(input_buffer.start_tick, None);
        assert_eq!(input_buffer.get(Tick(10)), None);
    }

    /// Characterization: `set_raw` repeats the last value over gaps and
    /// silently ignores ticks below `start_tick`.
    #[test]
    fn test_set_raw_gap_fill_and_below_start() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();
        input_buffer.set(Tick(10), 1);
        input_buffer.set_raw(Tick(13), Compressed::Input(2));
        assert_eq!(input_buffer.get(Tick(11)), Some(&1));
        assert_eq!(input_buffer.get(Tick(12)), Some(&1));
        assert_eq!(input_buffer.get(Tick(13)), Some(&2));
        input_buffer.set_raw(Tick(5), Compressed::Input(9));
        assert_eq!(input_buffer.start_tick, Some(Tick(10)));
        assert_eq!(input_buffer.get(Tick(5)), None);
    }

    /// Characterization: `pop_keeping_last` behaves like `pop` below the last tick.
    #[test]
    fn test_pop_keeping_last_below_end() {
        let mut input_buffer = InputBuffer::<i32, i32>::default();
        input_buffer.set(Tick(10), 1);
        input_buffer.set(Tick(11), 2);
        input_buffer.set(Tick(12), 3);
        assert_eq!(input_buffer.pop_keeping_last(Tick(10)), Some(1));
        assert_eq!(input_buffer.start_tick, Some(Tick(11)));
        assert_eq!(input_buffer.get(Tick(12)), Some(&3));
    }
}
