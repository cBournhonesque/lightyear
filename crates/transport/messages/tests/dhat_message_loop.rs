#![cfg(feature = "test_utils")]

use lightyear_messages::test_utils::{MessageQueueFixture, MessageSerializationFixture};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const MESSAGES_PER_BATCH: usize = 8;
const WARMUP_MESSAGES: usize = 100;
const MEASURED_MESSAGES: usize = 1_000;

#[test]
#[ignore = "manual heap profile; writes target/dhat-message-loop.json"]
fn dhat_message_loop() {
    let mut queue_fixture = MessageQueueFixture::default();
    let mut serialization_fixture = MessageSerializationFixture::default();

    queue_fixture.run_messages(WARMUP_MESSAGES, MESSAGES_PER_BATCH);
    serialization_fixture.run_messages(WARMUP_MESSAGES).unwrap();

    let profile_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/dhat-message-loop.json");
    std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
    let _profiler = dhat::Profiler::builder().file_name(&profile_path).build();

    let queue_stats = queue_fixture.run_messages(MEASURED_MESSAGES, MESSAGES_PER_BATCH);
    assert_eq!(queue_stats.messages, MEASURED_MESSAGES);

    let serialization_stats = serialization_fixture
        .run_messages(MEASURED_MESSAGES)
        .unwrap();
    assert_eq!(serialization_stats.messages, MEASURED_MESSAGES);
}
