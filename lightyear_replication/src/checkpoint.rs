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
//! Lightyear does not modify Replicon's internal message format. Instead, the Lightyear transport
//! bridge wraps Replicon server payloads on the wire:
//!
//! 1. On the server, [`wrap_server_payload`] prefixes the raw Replicon bytes with a lightweight
//!    Lightyear header containing the authoritative [`Tick`].
//! 2. On the client, [`unwrap_server_payload`] strips that header back off before forwarding the
//!    original bytes to Replicon.
//! 3. Before forwarding, [`extract_server_replicon_tick`] reads the Replicon checkpoint identifier
//!    from the inner payload and [`ReplicationCheckpointMap::record`] stores the
//!    `RepliconTick -> Tick` association.
//!
//! The wrapped bytes are only used at the Lightyear bridge boundary. Replicon still receives the
//! original payload unchanged.
//!
//! # Which tick to use
//! - Use [`RepliconTick`] for Replicon protocol concerns only: packet ordering, completeness, and
//!   decoding [`ConfirmHistory`] / [`ServerMutateTicks`].
//! - Use Lightyear [`Tick`] for prediction history, rollback decisions, mismatch comparisons, and
//!   anything that interacts with the local timeline or timeline resync.
//! - When code receives a tick from Replicon state, resolve it through
//!   [`ReplicationCheckpointMap`] before using it in prediction or rollback logic.
//!
//! [`ConfirmHistory`]: bevy_replicon::prelude::ConfirmHistory
//! [`ServerMutateTicks`]: bevy_replicon::prelude::ServerMutateTicks
use alloc::collections::VecDeque;
use bytes::{Buf, Bytes, BytesMut};

use bevy_ecs::prelude::Resource;
use bevy_platform::collections::HashMap;
use bevy_replicon::prelude::RepliconTick;
use lightyear_core::tick::Tick;

const CHECKPOINT_MAGIC: [u8; 2] = *b"LY";
const CHECKPOINT_VERSION: u8 = 1;
const HEADER_LEN: usize = 7;
const MAX_STORED_CHECKPOINTS: usize = 256;

/// Lightyear-owned header prepended to wrapped Replicon server payloads.
///
/// This header is carried only across the Lightyear transport bridge. Replicon never sees it:
/// the client unwraps the payload and forwards the original inner bytes to Replicon unchanged.
///
/// `authoritative_tick` is the server-side Lightyear simulation tick that the wrapped Replicon
/// checkpoint represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplicationCheckpointHeader {
    pub version: u8,
    pub authoritative_tick: Tick,
}

impl ReplicationCheckpointHeader {
    pub const fn new(authoritative_tick: Tick) -> Self {
        Self {
            version: CHECKPOINT_VERSION,
            authoritative_tick,
        }
    }
}

/// Receiver-side translation table from Replicon checkpoint ticks to authoritative server ticks.
///
/// This resource is populated when the client receives wrapped Replicon server packets. For each
/// payload on Replicon's update/mutation channels, the bridge:
///
/// 1. reads the Lightyear header to recover the authoritative server [`Tick`]
/// 2. reads the inner Replicon payload to recover the corresponding [`RepliconTick`]
/// 3. records the association here before handing the original bytes to Replicon
///
/// Prediction and rollback code should use this resource whenever they need to interpret
/// [`ConfirmHistory`] or [`ServerMutateTicks`] in simulation time.
///
/// This mapping is intentionally bounded because it is only needed for recent rollback /
/// prediction windows.
#[derive(Resource, Default, Debug)]
pub struct ReplicationCheckpointMap {
    entries: HashMap<RepliconTick, Tick>,
    order: VecDeque<RepliconTick>,
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

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointError {
    MissingHeader,
    InvalidMagic,
    UnsupportedVersion(u8),
    UnsupportedChannel(usize),
    InvalidPayload,
}

/// Prefix a Replicon server payload with the authoritative Lightyear simulation tick.
///
/// This is done in the Lightyear server bridge immediately before the payload is handed to the
/// transport layer. Replicon's serialized bytes are preserved verbatim after the header.
pub fn wrap_server_payload(authoritative_tick: Tick, payload: Bytes) -> Bytes {
    let mut wrapped = BytesMut::with_capacity(HEADER_LEN + payload.len());
    wrapped.extend_from_slice(&CHECKPOINT_MAGIC);
    wrapped.extend_from_slice(&[CHECKPOINT_VERSION]);
    wrapped.extend_from_slice(&authoritative_tick.0.to_le_bytes());
    wrapped.extend_from_slice(&payload);
    wrapped.freeze()
}

/// Remove the Lightyear checkpoint header from a wrapped Replicon server payload.
///
/// The returned [`Bytes`] are the original Replicon payload and should be forwarded to Replicon
/// unchanged.
pub fn unwrap_server_payload(
    payload: Bytes,
) -> Result<(ReplicationCheckpointHeader, Bytes), CheckpointError> {
    if payload.len() < HEADER_LEN {
        return Err(CheckpointError::MissingHeader);
    }
    if payload[0..2] != CHECKPOINT_MAGIC {
        return Err(CheckpointError::InvalidMagic);
    }
    let version = payload[2];
    if version != CHECKPOINT_VERSION {
        return Err(CheckpointError::UnsupportedVersion(version));
    }
    let authoritative_tick = Tick(u32::from_le_bytes([
        payload[3], payload[4], payload[5], payload[6],
    ]));
    Ok((
        ReplicationCheckpointHeader {
            version,
            authoritative_tick,
        },
        payload.slice(HEADER_LEN..),
    ))
}

/// Extract the Replicon checkpoint identifier from a wrapped server payload's inner bytes.
///
/// Replicon uses different payload layouts on its server update and mutation channels:
///
/// - channel `0` carries update packets whose leading Replicon tick is the checkpoint id
/// - channel `1` carries mutation packets with both `update_tick` and `message_tick`; the latter
///   is the completeness checkpoint used by [`ConfirmHistory`] / [`ServerMutateTicks`]
///
/// The returned [`RepliconTick`] should not be used directly for rollback or prediction. It is
/// only the lookup key into [`ReplicationCheckpointMap`].
pub fn extract_server_replicon_tick(
    channel_idx: usize,
    payload: &Bytes,
) -> Result<RepliconTick, CheckpointError> {
    let mut payload = payload.clone();
    match channel_idx {
        0 => {
            if payload.is_empty() {
                return Err(CheckpointError::InvalidPayload);
            }
            payload.advance(1);
            bevy_replicon::postcard_utils::from_buf(&mut payload)
                .map_err(|_| CheckpointError::InvalidPayload)
        }
        1 => {
            let _: RepliconTick = bevy_replicon::postcard_utils::from_buf(&mut payload)
                .map_err(|_| CheckpointError::InvalidPayload)?;
            let message_tick: RepliconTick = bevy_replicon::postcard_utils::from_buf(&mut payload)
                .map_err(|_| CheckpointError::InvalidPayload)?;
            Ok(message_tick)
        }
        _ => Err(CheckpointError::UnsupportedChannel(channel_idx)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    use bevy_replicon::postcard_utils;

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
    fn update_payload_roundtrip_preserves_bytes_and_tick() {
        let replicon_tick = RepliconTick::new(77);
        let mut payload = Vec::new();
        payload.push(0b0000_1000);
        postcard_utils::to_extend_mut(&replicon_tick, &mut payload).unwrap();
        payload.extend_from_slice(&[1, 2, 3, 4]);
        let payload = Bytes::from(payload);

        let wrapped = wrap_server_payload(Tick(42), payload.clone());
        let (header, inner) = unwrap_server_payload(wrapped).unwrap();

        assert_eq!(header, ReplicationCheckpointHeader::new(Tick(42)));
        assert_eq!(inner, payload);
        assert_eq!(
            extract_server_replicon_tick(0, &inner).unwrap(),
            replicon_tick
        );
    }

    #[test]
    fn mutation_payload_roundtrip_preserves_bytes_and_tick() {
        let update_tick = RepliconTick::new(10);
        let message_tick = RepliconTick::new(11);
        let mut payload = Vec::new();
        postcard_utils::to_extend_mut(&update_tick, &mut payload).unwrap();
        postcard_utils::to_extend_mut(&message_tick, &mut payload).unwrap();
        postcard_utils::to_extend_mut(&3usize, &mut payload).unwrap();
        payload.extend_from_slice(&[9, 8, 7]);
        let payload = Bytes::from(payload);

        let wrapped = wrap_server_payload(Tick(55), payload.clone());
        let (header, inner) = unwrap_server_payload(wrapped).unwrap();

        assert_eq!(header, ReplicationCheckpointHeader::new(Tick(55)));
        assert_eq!(inner, payload);
        assert_eq!(
            extract_server_replicon_tick(1, &inner).unwrap(),
            message_tick
        );
    }

    #[test]
    fn malformed_payload_is_rejected() {
        let payload = Bytes::from_static(b"bad");

        assert_eq!(
            unwrap_server_payload(payload),
            Err(CheckpointError::MissingHeader)
        );
    }
}
