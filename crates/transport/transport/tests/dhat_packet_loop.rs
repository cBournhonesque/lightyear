#![cfg(feature = "test_utils")]

use lightyear_transport::packet::test_utils::PacketLoopFixture;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const MESSAGES_PER_BATCH: usize = 4;
const PAYLOAD_BYTES: usize = 64;
const WARMUP_MESSAGES: usize = 1_500;
const MEASURED_MESSAGES: usize = 1_000;

#[test]
#[ignore = "manual heap profile; writes target/dhat-packet-loop.json"]
fn dhat_packet_send_receive_loop() {
    let mut fixture = PacketLoopFixture::new(MESSAGES_PER_BATCH, PAYLOAD_BYTES);

    let warmup = fixture.prepare_batches(WARMUP_MESSAGES);
    fixture.run_batches(warmup).unwrap();

    let batches = fixture.prepare_batches(MEASURED_MESSAGES);
    let profile_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/dhat-packet-loop.json");
    std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
    let _profiler = dhat::Profiler::builder().file_name(&profile_path).build();

    let stats = fixture.run_batches(batches).unwrap();
    assert_eq!(
        stats.packets,
        fixture.expected_packets_for_messages(MEASURED_MESSAGES)
    );
    assert_eq!(stats.messages, MEASURED_MESSAGES);
    assert_eq!(
        stats.payload_bytes,
        fixture.expected_payload_bytes_for_messages(MEASURED_MESSAGES)
    );
}
