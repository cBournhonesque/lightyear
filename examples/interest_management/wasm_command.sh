RUSTFLAGS=--cfg=web_sys_unstable_apis cargo build --release  --features webtransport --target wasm32-unknown-unknown
wasm-bindgen --no-typescript --target web \
  --out-dir ./wasm/ \
  --out-name "interest_management" \
  ../../target/wasm32-unknown-unknown/release/interest_management.wasm