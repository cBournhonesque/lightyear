use criterion::criterion_main;

mod message;

mod replication;
mod transport_compression;

criterion_main!(
    message::message_benches,
    replication::replication_benches,
    transport_compression::transport_compression_benches,
);
