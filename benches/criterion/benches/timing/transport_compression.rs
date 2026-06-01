use core::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group};
use lightyear_benches::transport_compression::{
    CompressionMode, TRANSPORT_COMPRESSION_CASES, prepare_transport_compression_case,
    print_transport_compression_stats_once, run_prepared_transport_compression_case,
};

criterion_group!(
    name = transport_compression_benches;
    config = Criterion::default();
    targets = send_receive_messages,
);

fn send_receive_messages(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("transport/compression/timing/send_receive_messages");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(100));
    group.measurement_time(Duration::from_millis(1000));

    for case in TRANSPORT_COMPRESSION_CASES {
        for mode in CompressionMode::ALL {
            print_transport_compression_stats_once(*case, mode);
            group.bench_with_input(
                BenchmarkId::new(case.name, mode.name()),
                &(*case, mode),
                |bencher, &(case, mode)| {
                    bencher.iter_batched(
                        || prepare_transport_compression_case(case, mode),
                        |mut prepared| {
                            let run = run_prepared_transport_compression_case(&mut prepared);
                            std::hint::black_box(run);
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}
