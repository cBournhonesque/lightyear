use criterion::criterion_main;

mod message;

mod replication;

criterion_main!(message::message_benches, replication::replication_benches,);
