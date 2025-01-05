# Rebuild the compiletime crate without rebuilding the dependencies
# Measure the time it takes to compile the crate
rm -rf "target/debug/.fingerprint/compiletime*" && rm -rf "target/debug/incremental/compiletime*" && rm -rf "target/debug/compiletime*" && cargo build -p compiletime