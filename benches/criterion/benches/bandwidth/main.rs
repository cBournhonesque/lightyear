use criterion::criterion_main;

mod replication;
mod transport_compression;

criterion_main!(
    replication::replication_bandwidth,
    transport_compression::transport_compression_bandwidth,
);
