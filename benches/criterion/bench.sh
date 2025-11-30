# Run the benchmark and generate a flamegraph.
# The env variables are needed to have debug symbols even in release mode.
CARGO_PROFILE_RELEASE_DEBUG=true RUSTFLAGS='-C force-frame-pointers=y' cargo bench --bench=replication --profile=release -- send_float_insert/1 --nocapture --profile-time=10

# Run the flamegraph separately
CARGO_PROFILE_RELEASE_DEBUG=true RUSTFLAGS='-C force-frame-pointers=y' cargo flamegraph --root --bin=replication_profiling --profile=release

#