use core::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group};
use lightyear_benches::measurements::bandwidth::Bandwidth;
use lightyear_benches::transport_compression::{
    CompressionMode, TRANSPORT_COMPRESSION_CASES, print_transport_compression_stats_once,
    run_transport_compression_case,
};

criterion_group!(
    name = transport_compression_bandwidth;
    config = Criterion::default().with_measurement(Bandwidth);
    targets = send_receive_messages,
);

fn send_receive_messages(criterion: &mut Criterion<Bandwidth>) {
    let mut group =
        criterion.benchmark_group("transport/compression/bandwidth/send_receive_messages");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(10));
    group.measurement_time(Duration::from_millis(10));

    for case in TRANSPORT_COMPRESSION_CASES {
        for mode in CompressionMode::ALL {
            print_transport_compression_stats_once(*case, mode);
            group.bench_with_input(
                BenchmarkId::new(case.name, mode.name()),
                &(*case, mode),
                |bencher, &(case, mode)| {
                    bencher.iter_custom(|iter| {
                        let mut total = 0.0;
                        for _ in 0..iter {
                            let run = run_transport_compression_case(case, mode);
                            std::hint::black_box(run.compression_above_mtu_packets);
                            total += std::hint::black_box(run.send_bytes);
                        }
                        total
                    });
                },
            );
        }
    }

    group.finish();
}
