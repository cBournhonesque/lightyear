//! Maps Replicon delivery checkpoints onto Lightyear simulation time.
//!
//! # Why this exists
//! Replicon and Lightyear use different notions of "tick":
//!
//! - [`RepliconTick`] is a replication checkpoint / message-ordering index. It is owned by
//!   Replicon and is what [`ConfirmHistory`] and [`ServerMutateTicks`] report.
//!   [`RepliconTick`] is incremented by one whenever the 'send' side of replication runs.
//! - [`Tick`] is Lightyear's authoritative simulation tick. Prediction history, rollback,
//!   timeline resync, and mismatch detection must operate in this tick domain.
//!   [`Tick`] is incremented by one whenever the FixedUpdate schedule runs.
//!
//! The bridge therefore carries an explicit mapping from each received Replicon checkpoint to the
//! authoritative server simulation.
//!
//! # How the mapping is carried
//! Lightyear stores the authoritative [`Tick`] in Replicon's replication userdata:
//!
//! 1. On the server, [`write_authoritative_tick_userdata`] writes the current Lightyear tick into
//!    [`ReplicationUserdata`](bevy_replicon::server::ReplicationUserdata) before Replicon builds
//!    replication messages.
//! 2. Replicon serializes those bytes into its update and mutation messages.
//! 3. On the client, Replicon triggers
//!    [`UserdataReceived`](bevy_replicon::client::UserdataReceived) before applying the message,
//!    and [`record_authoritative_tick_userdata`] stores the `RepliconTick -> Tick` association.
//!
//! # Which tick to use
//! - Use [`RepliconTick`] for Replicon protocol concerns only: packet ordering, completeness, and
//!   decoding [`ConfirmHistory`] / [`ServerMutateTicks`].
//! - Use Lightyear [`Tick`] for prediction history, rollback decisions, mismatch comparisons, and
//!   anything that interacts with the local timeline or timeline resync.
//! - When code receives a tick from Replicon state, resolve it through
//!   [`ReplicationCheckpointMap`] before using it in prediction or rollback logic.
//!
//! [`ConfirmHistory`]: bevy_replicon::client::confirm_history::ConfirmHistory
//! [`ServerMutateTicks`]: bevy_replicon::client::server_mutate_ticks::ServerMutateTicks
use alloc::collections::VecDeque;

#[cfg(feature = "client")]
use bevy_ecs::prelude::On;
#[cfg(feature = "server")]
use bevy_ecs::prelude::Res;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::prelude::ResMut;
use bevy_ecs::prelude::Resource;
use bevy_platform::collections::HashMap;
#[cfg(feature = "client")]
use bevy_replicon::client::UserdataReceived;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
use bevy_replicon::prelude::RepliconTick;
#[cfg(feature = "server")]
use bevy_replicon::server::ReplicationUserdata;
#[cfg(feature = "server")]
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::Tick;
#[cfg(feature = "client")]
use tracing::error;

pub const CHECKPOINT_USERDATA_LEN: usize = core::mem::size_of::<Tick>();
const MAX_STORED_CHECKPOINTS: usize = 256;

/// Receiver-side translation table from Replicon checkpoint ticks to authoritative server ticks.
///
/// This resource is populated when Replicon triggers
/// [`UserdataReceived`](bevy_replicon::client::UserdataReceived). For each payload on Replicon's
/// update/mutation channels, the bridge:
///
/// 1. reads the Lightyear [`Tick`] from the Replicon userdata bytes
/// 2. uses Replicon's `message_tick` as the corresponding [`RepliconTick`]
/// 3. records the association here before Replicon applies the message
///
/// Prediction and rollback code should use this resource whenever they need to interpret
/// [`ConfirmHistory`] or
/// [`ServerMutateTicks`](bevy_replicon::client::server_mutate_ticks::ServerMutateTicks) in
/// simulation time.
///
/// This mapping is intentionally bounded because it is only needed for recent rollback /
/// prediction windows.
#[derive(Resource, Default, Debug)]
pub struct ReplicationCheckpointMap {
    entries: HashMap<RepliconTick, Tick>,
    order: VecDeque<RepliconTick>,
    last_confirmed_checkpoint: Option<CompletedCheckpoint>,
}

/// A completed Replicon checkpoint resolved into Lightyear simulation time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletedCheckpoint {
    /// Replicon's protocol tick for this completed checkpoint.
    pub replicon_tick: RepliconTick,
    /// Authoritative Lightyear simulation tick carried by the checkpoint.
    pub tick: Tick,
}

impl ReplicationCheckpointMap {
    /// Record the authoritative server simulation tick for a Replicon checkpoint.
    ///
    /// Multiple Replicon ticks may legitimately map to the same authoritative [`Tick`]. That
    /// happens when several replication sends are produced for the same simulation step.
    pub fn record(&mut self, replicon_tick: RepliconTick, authoritative_tick: Tick) {
        if !self.entries.contains_key(&replicon_tick) {
            self.order.push_back(replicon_tick);
            if self.order.len() > MAX_STORED_CHECKPOINTS
                && let Some(oldest) = self.order.pop_front()
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(replicon_tick, authoritative_tick);
    }

    /// Resolve a Replicon checkpoint into authoritative server simulation time.
    ///
    /// Use this before inserting confirmed values into prediction history or before comparing
    /// Replicon confirmation metadata against the local prediction timeline.
    pub fn get(&self, replicon_tick: RepliconTick) -> Option<Tick> {
        self.entries.get(&replicon_tick).copied()
    }

    /// Latest authoritative Lightyear tick for which Replicon completed all mutate messages.
    pub fn last_confirmed_tick(&self) -> Option<Tick> {
        self.last_confirmed_checkpoint
            .map(|checkpoint| checkpoint.tick)
    }

    /// Latest checkpoint for which Replicon completed all mutate messages.
    pub fn last_confirmed_checkpoint(&self) -> Option<CompletedCheckpoint> {
        self.last_confirmed_checkpoint
    }

    /// Return the newest completed checkpoint that is not ahead of `limit`.
    ///
    /// The common case uses the cached latest completed checkpoint. When that checkpoint is ahead
    /// of the local timeline, inspect the retained Replicon-to-Lightyear mappings and keep only
    /// ticks that [`ServerMutateTicks`] reports as complete. Multiple Replicon ticks can map to the
    /// same Lightyear tick; ties prefer the most recent Replicon tick.
    pub fn latest_completed_at_or_before(
        &self,
        server_mutate_ticks: &ServerMutateTicks,
        limit: Tick,
    ) -> Option<CompletedCheckpoint> {
        if let Some(checkpoint) = self.last_confirmed_checkpoint
            && checkpoint.tick <= limit
        {
            return Some(checkpoint);
        }

        self.latest_completed_matching(server_mutate_ticks, |tick| tick <= limit)
    }

    /// Return the newest completed checkpoint strictly before `limit`.
    ///
    /// This is used when an unresolved diff at `limit` makes that tick and every later tick
    /// unusable as a rollback checkpoint. The returned checkpoint is selected only from mappings
    /// that [`ServerMutateTicks`] reports as globally complete.
    pub fn latest_completed_before(
        &self,
        server_mutate_ticks: &ServerMutateTicks,
        limit: Tick,
    ) -> Option<CompletedCheckpoint> {
        if let Some(checkpoint) = self.last_confirmed_checkpoint
            && checkpoint.tick < limit
        {
            return Some(checkpoint);
        }

        self.latest_completed_matching(server_mutate_ticks, |tick| tick < limit)
    }

    fn latest_completed_matching(
        &self,
        server_mutate_ticks: &ServerMutateTicks,
        is_within_bound: impl Fn(Tick) -> bool,
    ) -> Option<CompletedCheckpoint> {
        let mut best = None;
        for (&replicon_tick, &tick) in &self.entries {
            if !server_mutate_ticks.contains(replicon_tick) || !is_within_bound(tick) {
                continue;
            }
            let candidate = CompletedCheckpoint {
                replicon_tick,
                tick,
            };
            if best.is_none_or(|existing: CompletedCheckpoint| {
                candidate.tick > existing.tick
                    || (candidate.tick == existing.tick
                        && replicon_tick.is_newer(existing.replicon_tick))
            }) {
                best = Some(candidate);
            }
        }
        best
    }

    /// Resolve a completed Replicon mutate checkpoint and update the latest confirmed checkpoint.
    ///
    /// A checkpoint advances the cache when its Lightyear tick is newer. If multiple Replicon
    /// checkpoints map to the same Lightyear tick, the newer Replicon checkpoint wins. Returns the
    /// resolved checkpoint, even when an already-cached checkpoint remains newer, or `None` if the
    /// mapping has not been received yet.
    pub fn record_last_confirmed_checkpoint(
        &mut self,
        replicon_tick: RepliconTick,
    ) -> Option<CompletedCheckpoint> {
        let checkpoint = CompletedCheckpoint {
            replicon_tick,
            tick: self.get(replicon_tick)?,
        };
        if self.last_confirmed_checkpoint.is_none_or(|existing| {
            checkpoint.tick > existing.tick
                || (checkpoint.tick == existing.tick
                    && checkpoint.replicon_tick.is_newer(existing.replicon_tick))
        }) {
            self.last_confirmed_checkpoint = Some(checkpoint);
        }
        Some(checkpoint)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.last_confirmed_checkpoint = None;
    }
}

/// Resolve a Replicon message tick into authoritative server simulation time.
pub fn resolve_message_tick(
    checkpoints: &ReplicationCheckpointMap,
    message_tick: RepliconTick,
) -> Option<Tick> {
    checkpoints.get(message_tick)
}

/// Resolve an entity's latest Replicon confirmation into authoritative server simulation time.
pub fn resolve_confirm_history_tick(
    checkpoints: &ReplicationCheckpointMap,
    confirm_history: &ConfirmHistory,
) -> Option<Tick> {
    checkpoints.get(confirm_history.last_tick())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointError {
    InvalidUserdataLength(usize),
}

pub fn encode_authoritative_tick(authoritative_tick: Tick) -> [u8; CHECKPOINT_USERDATA_LEN] {
    authoritative_tick.0.to_le_bytes()
}

pub fn decode_authoritative_tick(bytes: &[u8]) -> Result<Tick, CheckpointError> {
    if bytes.len() != CHECKPOINT_USERDATA_LEN {
        return Err(CheckpointError::InvalidUserdataLength(bytes.len()));
    }
    Ok(Tick(u32::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
    ])))
}

/// Write the authoritative Lightyear simulation tick into Replicon's replication userdata.
#[cfg(feature = "server")]
pub(crate) fn write_authoritative_tick_userdata(
    timeline: Res<LocalTimeline>,
    mut userdata: ResMut<ReplicationUserdata>,
) {
    userdata.clear();
    userdata.extend_from_slice(&encode_authoritative_tick(timeline.tick()));
}

/// Record the authoritative Lightyear simulation tick carried by Replicon userdata.
#[cfg(feature = "client")]
pub(crate) fn record_authoritative_tick_userdata(
    received: On<UserdataReceived>,
    mut checkpoints: ResMut<ReplicationCheckpointMap>,
) {
    match decode_authoritative_tick(received.bytes.as_ref()) {
        Ok(authoritative_tick) => checkpoints.record(received.message_tick, authoritative_tick),
        Err(error_kind) => {
            error!(
                ?error_kind,
                replicon_tick = ?received.message_tick,
                userdata_len = received.bytes.len(),
                "dropping invalid replicon checkpoint userdata"
            );
            debug_assert!(
                false,
                "invalid authoritative checkpoint userdata: {error_kind:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_map_prunes_old_entries() {
        let mut map = ReplicationCheckpointMap::default();

        for i in 0..(MAX_STORED_CHECKPOINTS as u32 + 1) {
            map.record(RepliconTick::new(i), Tick::from(i));
        }

        assert_eq!(map.get(RepliconTick::new(0)), None);
        assert_eq!(
            map.get(RepliconTick::new(MAX_STORED_CHECKPOINTS as u32)),
            Some(Tick::from(MAX_STORED_CHECKPOINTS as u32))
        );
    }

    #[test]
    fn checkpoint_map_records_latest_confirmed_checkpoint() {
        let mut map = ReplicationCheckpointMap::default();
        map.record(RepliconTick::new(9), Tick(90));
        map.record(RepliconTick::new(10), Tick(100));
        map.record(RepliconTick::new(11), Tick(100));

        assert_eq!(
            map.record_last_confirmed_checkpoint(RepliconTick::new(10)),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(10),
                tick: Tick(100),
            })
        );
        assert_eq!(
            map.record_last_confirmed_checkpoint(RepliconTick::new(9)),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(9),
                tick: Tick(90),
            })
        );
        assert_eq!(
            map.record_last_confirmed_checkpoint(RepliconTick::new(11)),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(11),
                tick: Tick(100),
            })
        );
        map.record_last_confirmed_checkpoint(RepliconTick::new(10));
        assert_eq!(
            map.last_confirmed_checkpoint(),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(11),
                tick: Tick(100),
            })
        );
        assert_eq!(map.last_confirmed_tick(), Some(Tick(100)));
    }

    #[test]
    fn checkpoint_map_finds_latest_completed_tick_before_limit() {
        let mut map = ReplicationCheckpointMap::default();
        let mut server_ticks = ServerMutateTicks::default();
        for (replicon, authoritative) in [(8, 80), (9, 90), (10, 100)] {
            let replicon_tick = RepliconTick::new(replicon);
            map.record(replicon_tick, Tick(authoritative));
            server_ticks.confirm(replicon_tick, 1);
        }
        map.record_last_confirmed_checkpoint(RepliconTick::new(10));

        assert_eq!(
            map.latest_completed_at_or_before(&server_ticks, Tick(95)),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(9),
                tick: Tick(90),
            })
        );
        assert_eq!(
            map.latest_completed_at_or_before(&server_ticks, Tick(100)),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(10),
                tick: Tick(100),
            })
        );
        assert_eq!(
            map.latest_completed_before(&server_ticks, Tick(100)),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(9),
                tick: Tick(90),
            })
        );
    }

    #[test]
    fn checkpoint_map_ignores_mapped_but_incomplete_ticks() {
        let mut map = ReplicationCheckpointMap::default();
        let mut server_ticks = ServerMutateTicks::default();

        map.record(RepliconTick::new(8), Tick(80));
        server_ticks.confirm(RepliconTick::new(8), 1);
        map.record_last_confirmed_checkpoint(RepliconTick::new(8));

        // Userdata is recorded before Replicon applies the message. A mapping can therefore exist
        // before all mutation messages for its checkpoint have arrived.
        map.record(RepliconTick::new(9), Tick(90));

        // Put the cached frontier beyond the lookup bound so the historical lookup has to inspect
        // both the completed and incomplete retained mappings.
        map.record(RepliconTick::new(10), Tick(110));
        server_ticks.confirm(RepliconTick::new(10), 1);
        map.record_last_confirmed_checkpoint(RepliconTick::new(10));

        assert_eq!(
            map.latest_completed_at_or_before(&server_ticks, Tick(100)),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(8),
                tick: Tick(80),
            })
        );
    }

    #[test]
    fn checkpoint_map_prefers_newest_replicon_tick_for_same_lightyear_tick() {
        let mut map = ReplicationCheckpointMap::default();
        let mut server_ticks = ServerMutateTicks::default();
        for replicon in [8, 9] {
            let replicon_tick = RepliconTick::new(replicon);
            map.record(replicon_tick, Tick(80));
            server_ticks.confirm(replicon_tick, 1);
        }
        map.record(RepliconTick::new(10), Tick(100));
        server_ticks.confirm(RepliconTick::new(10), 1);
        map.record_last_confirmed_checkpoint(RepliconTick::new(10));

        assert_eq!(
            map.latest_completed_at_or_before(&server_ticks, Tick(80)),
            Some(CompletedCheckpoint {
                replicon_tick: RepliconTick::new(9),
                tick: Tick(80),
            })
        );
    }

    #[test]
    fn checkpoint_map_missing_confirmed_tick_does_not_update_cache() {
        let mut map = ReplicationCheckpointMap::default();

        assert_eq!(
            map.record_last_confirmed_checkpoint(RepliconTick::new(10)),
            None
        );
        assert_eq!(map.last_confirmed_tick(), None);
        assert_eq!(map.last_confirmed_checkpoint(), None);
    }

    #[test]
    fn checkpoint_map_clear_resets_confirmed_checkpoint_cache() {
        let mut map = ReplicationCheckpointMap::default();
        map.record(RepliconTick::new(10), Tick(100));
        map.record_last_confirmed_checkpoint(RepliconTick::new(10));

        map.clear();

        assert_eq!(map.get(RepliconTick::new(10)), None);
        assert_eq!(map.last_confirmed_tick(), None);
        assert_eq!(map.last_confirmed_checkpoint(), None);
    }

    #[test]
    fn authoritative_tick_userdata_roundtrips() {
        let bytes = encode_authoritative_tick(Tick(42));

        assert_eq!(decode_authoritative_tick(&bytes), Ok(Tick(42)));
    }

    #[test]
    fn invalid_authoritative_tick_userdata_is_rejected() {
        assert_eq!(
            decode_authoritative_tick(&[1, 2, 3]),
            Err(CheckpointError::InvalidUserdataLength(3))
        );
    }
}
