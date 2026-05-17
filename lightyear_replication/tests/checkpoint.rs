use alloc::vec::Vec;
use bytes::Bytes;
use lightyear_core::tick::Tick;
use lightyear_replication::checkpoint::{
    CheckpointError, ReplicationCheckpointHeader, ReplicationCheckpointMap,
    extract_server_replicon_tick, unwrap_server_payload, wrap_server_payload,
};

use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::RepliconTick;

extern crate alloc;

#[test]
fn checkpoint_map_prunes_old_entries() {
    let mut map = ReplicationCheckpointMap::default();

    for i in 0..257u32 {
        map.record(RepliconTick::new(i), Tick::from(i));
    }

    assert_eq!(map.get(RepliconTick::new(0)), None);
    assert_eq!(map.get(RepliconTick::new(256)), Some(Tick::from(256)));
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
