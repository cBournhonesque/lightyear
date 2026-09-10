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
#[allow(unused_imports)]
use tracing::{info, trace};

/// Maximum number of ticks retained in an [`InputBuffer`].
pub const INPUT_BUFFER_CAPACITY: usize = 64;

/// Buffer that stores a value (usually Inputs) for the last few ticks.
///
/// S is the type of the InputSnapshot.
/// M is present in case the InputSnapshot does not have a generic.
#[derive(Component, Reflect)]
pub struct InputBuffer<S, M> {
    pub start_tick: Option<Tick>,
    /// Fixed ring holding the window `[start_tick, start_tick + len)`, oldest first.
    slots: [Option<S>; INPUT_BUFFER_CAPACITY],
    /// Ring index of `start_tick`.
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

impl<T: Clone + PartialEq, M> InputBuffer<T, M> {
    /// Ring index for `tick`. `None` when `tick` is outside `[start_tick, end_tick]`.
    fn index(&self, tick: Tick) -> Option<usize> {
        let start_tick = self.start_tick?;
        // `Tick - Tick` is an `i32`, negative when `tick < start_tick`.
        let offset = tick - start_tick;
        if offset < 0 || self.is_empty() {
            return None;
        }
        let offset = offset as usize;
        if offset >= self.len {
            return None;
        }
        Some((self.head + offset) % INPUT_BUFFER_CAPACITY)
    }

    /// Append a slot at the back, evicting the oldest tick once the window is full.
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

    /// Whether the buffer holds no ticks
    pub fn is_empty(&self) -> bool {
        self.len == 0
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
        self.set_raw(tick, Some(value));
    }

    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    /// Set the ActionState for the given tick in the InputBuffer
    ///
    /// This should be called every tick.
    pub fn set_empty(&mut self, tick: Tick) {
        self.set_raw(tick, None);
    }

    /// Write one materialized tick.
    ///
    /// Gaps between the old end and `tick` repeat the last stored value
    /// (hold-last); ticks below `start_tick` are ignored.
    pub fn set_raw(&mut self, tick: Tick, value: Option<T>) {
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.head = 0;
            self.len = 0;
            self.push_back(value);
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
            // Policy: a missing tick repeats the last stored action
            // (RocketLeague/Overwatch hold-last). The copies keep the
            // anchor's timers: each tick past the anchor is decayed exactly
            // once, at read time inside predict(), so re-predicting is
            // idempotent and stored values stay exact.
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
        self.slots[index] = value;
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

    /// Get latest ActionState present in the buffer
    pub fn get_last(&self) -> Option<&T> {
        let start_tick = self.start_tick?;
        if self.is_empty() {
            return None;
        }
        self.get(start_tick + (self.len as i32 - 1))
    }

    /// Get latest ActionState present in the buffer, along with the associated Tick
    pub fn get_last_with_tick(&self) -> Option<(Tick, &T)> {
        let start_tick = self.start_tick?;
        if self.is_empty() {
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
        input_buffer.set_raw(Tick(2), Some(7));
        assert_eq!(input_buffer.get(Tick(2)), Some(&7));
        // neutral overwrites a stored value
        input_buffer.set_raw(Tick(2), None);
        assert_eq!(input_buffer.get(Tick(2)), None);
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
        input_buffer.set_raw(Tick(13), Some(2));
        assert_eq!(input_buffer.get(Tick(11)), Some(&1));
        assert_eq!(input_buffer.get(Tick(12)), Some(&1));
        assert_eq!(input_buffer.get(Tick(13)), Some(&2));
        input_buffer.set_raw(Tick(5), Some(9));
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
