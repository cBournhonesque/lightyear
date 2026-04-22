use alloc::collections::VecDeque;
use bytes::{Buf, Bytes, BytesMut};

use bevy_ecs::prelude::Resource;
use bevy_platform::collections::HashMap;
use bevy_replicon::prelude::RepliconTick;
use lightyear_core::tick::Tick;

const CHECKPOINT_MAGIC: [u8; 2] = *b"LY";
const CHECKPOINT_VERSION: u8 = 1;
const HEADER_LEN: usize = 5;
const MAX_STORED_CHECKPOINTS: usize = 256;

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

#[derive(Resource, Default, Debug)]
pub struct ReplicationCheckpointMap {
    entries: HashMap<RepliconTick, Tick>,
    order: VecDeque<RepliconTick>,
}

impl ReplicationCheckpointMap {
    pub fn record(&mut self, replicon_tick: RepliconTick, authoritative_tick: Tick) {
        if !self.entries.contains_key(&replicon_tick) {
            self.order.push_back(replicon_tick);
            if self.order.len() > MAX_STORED_CHECKPOINTS {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
        }
        self.entries.insert(replicon_tick, authoritative_tick);
    }

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

pub fn wrap_server_payload(authoritative_tick: Tick, payload: Bytes) -> Bytes {
    let mut wrapped = BytesMut::with_capacity(HEADER_LEN + payload.len());
    wrapped.extend_from_slice(&CHECKPOINT_MAGIC);
    wrapped.extend_from_slice(&[CHECKPOINT_VERSION]);
    wrapped.extend_from_slice(&authoritative_tick.0.to_le_bytes());
    wrapped.extend_from_slice(&payload);
    wrapped.freeze()
}

pub fn unwrap_server_payload(payload: Bytes) -> Result<(ReplicationCheckpointHeader, Bytes), CheckpointError> {
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
    let authoritative_tick = Tick(u16::from_le_bytes([payload[3], payload[4]]));
    Ok((
        ReplicationCheckpointHeader {
            version,
            authoritative_tick,
        },
        payload.slice(HEADER_LEN..),
    ))
}

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
        assert_eq!(extract_server_replicon_tick(0, &inner).unwrap(), replicon_tick);
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
        assert_eq!(extract_server_replicon_tick(1, &inner).unwrap(), message_tick);
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
