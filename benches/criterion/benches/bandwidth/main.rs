use criterion::criterion_main;

mod replication;

criterion_main!(replication::replication_bandwidth);
