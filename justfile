default: format taplo typos clippy clippy_examples test

# Local CI
format:
    cargo fmt

taplo:
    taplo fmt

typos:
    typos -w

doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --keep-going --all-features

clippy: lightyear lightyear_aeronet lightyear_avian lightyear_connection lightyear_core \
    lightyear_crossbeam lightyear_frame_interpolation lightyear_inputs lightyear_interpolation \
    lightyear_link lightyear_messages lightyear_netcode lightyear_prediction lightyear_replication \
    lightyear_serde lightyear_sync lightyear_tests lightyear_transport lightyear_udp lightyear_utils lightyear_webtransport
    # You can't use `--all-features` because of conflict between `avian2d` and `avian3d`.
    # cargo clippy --workspace --examples --tests --all-features -- -D warnings

clippy_examples:
    cargo clippy -p spaceships --all-features -- -D warnings --no-deps
    cargo clippy -p auth --all-features -- -D warnings --no-deps
    cargo clippy -p avian_3d_character --all-features -- -D warnings --no-deps
    cargo clippy -p avian_physics --all-features -- -D warnings --no-deps
    cargo clippy -p bevy_enhanced_inputs --all-features -- -D warnings --no-deps
    cargo clippy -p client_replication --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_examples_common --all-features -- -D warnings --no-deps
    # cargo clippy -p delta_compression --all-features -- -D warnings --no-deps
    # cargo clippy -p distributed_authority --all-features -- -D warnings --no-deps
    cargo clippy -p fps --all-features -- -D warnings --no-deps
    cargo clippy -p launcher --all-features -- -D warnings --no-deps
    cargo clippy -p lobby --all-features -- -D warnings --no-deps
    cargo clippy -p network_visibility --all-features -- -D warnings --no-deps
    cargo clippy -p priority --all-features -- -D warnings --no-deps
    cargo clippy -p replication_groups --all-features -- -D warnings --no-deps
    cargo clippy -p simple_box --all-features -- -D warnings --no-deps
    # cargo clippy -p simple_setup --all-features -- -D warnings --no-deps

test:
    # Can´t do --workspace because of feature unification with the packages in examples.
    # You can't use `--all-features` because of conflict between `avian2d` and `avian3d`.
    cargo test -p lightyear --no-default-features --features="std client server replication \
    interpolation trace metrics netcode webtransport webtransport_self_signed webtransport_dangerous_configuration \
    input_native leafwing input_bei avian2d udp websocket crossbeam steam"
    cargo test -p lightyear --no-default-features --features="std client server replication \
    interpolation trace metrics netcode webtransport webtransport_self_signed webtransport_dangerous_configuration \
    input_native leafwing input_bei avian3d udp websocket crossbeam steam"
    cargo test -p lightyear_aeronet --all-features
    # You can't use `--all-features` because of conflict between `avian2d` and `avian3d`.
    cargo test -p lightyear_avian --no-default-features --features="std 2d lag_compensation"
    cargo test -p lightyear_avian --no-default-features --features="std 3d lag_compensation"
    cargo test -p lightyear_connection --all-features
    cargo test -p lightyear_core --all-features
    cargo test -p lightyear_crossbeam --all-features
    cargo test -p lightyear_frame_interpolation --all-features
    cargo test -p lightyear_inputs --all-features
    cargo test -p lightyear_inputs_bei --all-features
    cargo test -p lightyear_inputs_leafwing --all-features
    cargo test -p lightyear_inputs_native --all-features
    cargo test -p lightyear_interpolation --all-features
    cargo test -p lightyear_link --all-features
    cargo test -p lightyear_messages --all-features
    cargo test -p lightyear_netcode --all-features
    cargo test -p lightyear_prediction --all-features
    cargo test -p lightyear_replication --all-features
    cargo test -p lightyear_serde --all-features
    cargo test -p lightyear_steam --all-features
    cargo test -p lightyear_sync --all-features
    cargo test -p lightyear_transport --all-features
    cargo test -p lightyear_udp --all-features
    cargo test -p lightyear_utils --all-features
    cargo test -p lightyear_webtransport --all-features
    # Limit to 1 test thread to prevent mocked GlobalTime from going crazy
    cargo test -p lightyear_tests --all-features -- --test-threads=1

# Clippy
lightyear:
    # `lightyear_avian` only works on `std`
    # cargo clippy -p lightyear --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std replication" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std prediction" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std interpolation" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std trace" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std netcode" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="webtransport" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std webtransport_self_signed" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std webtransport_dangerous_configuration" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std input_native" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std leafwing" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std input_bei" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="avian2d" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="avian3d" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="udp" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="websocket" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std crossbeam" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="steam" -- -D warnings --no-deps
    # You can't use `--all-features` because of conflict between `avian2d` and `avian3d`
    cargo clippy -p lightyear --no-default-features --features="std client server replication \
    interpolation trace metrics netcode webtransport webtransport_self_signed webtransport_dangerous_configuration \
    input_native leafwing input_bei avian2d udp websocket crossbeam steam" -- -D warnings --no-deps
    cargo clippy -p lightyear --no-default-features --features="std client server replication \
    interpolation trace metrics netcode webtransport webtransport_self_signed webtransport_dangerous_configuration \
    input_native leafwing input_bei avian3d udp websocket crossbeam steam" -- -D warnings --no-deps
    # cargo clippy -p lightyear --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std replication" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std prediction" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std interpolation" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std trace" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std netcode" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="webtransport" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std webtransport_self_signed" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std webtransport_dangerous_configuration" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std input_native" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std leafwing" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std input_bei" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="avian2d" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="avian3d" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="udp" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="websocket" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std crossbeam" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="steam" -- -D warnings --no-deps
    # You can't use `--all-features` because of conflict between `avian2d` and `avian3d`
    cargo clippy -p lightyear --tests --no-default-features --features="std client server replication \
    interpolation trace metrics netcode webtransport webtransport_self_signed webtransport_dangerous_configuration \
    input_native leafwing input_bei avian2d udp websocket crossbeam steam" -- -D warnings --no-deps
    cargo clippy -p lightyear --tests --no-default-features --features="std client server replication \
    interpolation trace metrics netcode webtransport webtransport_self_signed webtransport_dangerous_configuration \
    input_native leafwing input_bei avian3d udp websocket crossbeam steam" -- -D warnings --no-deps

lightyear_aeronet:
    cargo clippy -p lightyear_aeronet --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_aeronet --no-default-features --features="test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_aeronet --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_aeronet --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_aeronet --tests --no-default-features --features="test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_aeronet --tests --all-features -- -D warnings --no-deps

lightyear_avian:
    # `lightyear_avian` only works on `std`
    # `2d` and `3d` are mutually exclusive
    # `lag_compensation` requires either `2d` or `3d`
    cargo clippy -p lightyear_avian --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_avian --no-default-features --features="std 2d" -- -D warnings --no-deps
    cargo clippy -p lightyear_avian --no-default-features --features="std 3d" -- -D warnings --no-deps
    cargo clippy -p lightyear_avian --no-default-features --features="std 2d lag_compensation" -- -D warnings --no-deps
    cargo clippy -p lightyear_avian --no-default-features --features="std 3d lag_compensation" -- -D warnings --no-deps
    # cargo clippy -p lightyear_avian --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_avian --tests --no-default-features --features="std 2d" -- -D warnings --no-deps
    cargo clippy -p lightyear_avian --tests --no-default-features --features="std 3d" -- -D warnings --no-deps
    cargo clippy -p lightyear_avian --tests --no-default-features --features="std 2d lag_compensation" -- -D warnings --no-deps
    cargo clippy -p lightyear_avian --tests --no-default-features --features="std 3d lag_compensation" -- -D warnings --no-deps

lightyear_connection:
    cargo clippy -p lightyear_connection --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_connection --no-default-features --features="client" -- -D warnings --no-deps
    cargo clippy -p lightyear_connection --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_connection --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_connection --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_connection --tests --no-default-features --features="client" -- -D warnings --no-deps
    cargo clippy -p lightyear_connection --tests --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_connection --tests --all-features -- -D warnings --no-deps

lightyear_core:
    cargo clippy -p lightyear_core --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_core --no-default-features --features="prediction" -- -D warnings --no-deps
    cargo clippy -p lightyear_core --no-default-features --features="interpolation" -- -D warnings --no-deps
    cargo clippy -p lightyear_core --no-default-features --features="test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_core --no-default-features --features="not_mock" -- -D warnings --no-deps
    cargo clippy -p lightyear_core --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_core --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_core --tests --no-default-features --features="prediction" -- -D warnings --no-deps
    cargo clippy -p lightyear_core --tests --no-default-features --features="interpolation" -- -D warnings --no-deps
    cargo clippy -p lightyear_core --tests --no-default-features --features="test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_core --tests --no-default-features --features="not_mock" -- -D warnings --no-deps
    cargo clippy -p lightyear_core --tests --all-features -- -D warnings --no-deps

lightyear_crossbeam:
    cargo clippy -p lightyear_crossbeam --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_crossbeam --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_crossbeam --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_crossbeam --tests --all-features -- -D warnings --no-deps

lightyear_frame_interpolation:
    # `lightyear_frame_interpolation` only works with `std`
    # cargo clippy -p lightyear_frame_interpolation --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_frame_interpolation --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_frame_interpolation --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_frame_interpolation --tests --no-default-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_frame_interpolation --tests --no-default-features --features="std" -- -D warnings --no-deps
    # cargo clippy -p lightyear_frame_interpolation --tests --all-features -- -D warnings --no-deps
    
lightyear_inputs:
    # `lightyear_inputs` only works with `std`
    # cargo clippy -p lightyear_inputs --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --no-default-features --features="std interpolation" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --no-default-features --features="std interpolation client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --no-default-features --features="std interpolation server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --all-features -- -D warnings --no-deps
    # # cargo clippy -p lightyear_inputs --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --tests --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --tests --no-default-features --features="std interpolation" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --tests --no-default-features --features="std interpolation client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --tests --no-default-features --features="std interpolation server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs --tests --all-features -- -D warnings --no-deps

lightyear_inputs_bei:
    # `lightyear_inputs_bei` only works with `std`
    # cargo clippy -p lightyear_inputs_bei --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_bei --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_bei --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_bei --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_bei --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_inputs_bei --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_bei --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_bei --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_bei --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_bei --tests --all-features -- -D warnings --no-deps

lightyear_inputs_leafwing:
    # `lightyear_inputs_leafwing` only works with `std`
    cargo clippy -p lightyear_inputs_leafwing --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --no-default-features --features="client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --tests --no-default-features --features="client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --tests --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_leafwing --tests --all-features -- -D warnings --no-deps

lightyear_inputs_native:
    # `lightyear_inputs_native` only works with `std`
    # cargo clippy -p lightyear_inputs_native --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_native --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_native --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_native --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_native --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_inputs_native --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_native --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_native --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_native --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_inputs_native --tests --all-features -- -D warnings --no-deps

lightyear_interpolation:
    # `lightyear_interpolation` only works with `std`
    # cargo clippy -p lightyear_interpolation --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_interpolation --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_interpolation --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_interpolation --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_interpolation --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_interpolation --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_interpolation --tests --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_interpolation --tests --all-features -- -D warnings --no-deps

lightyear_link:
    cargo clippy -p lightyear_link --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_link --no-default-features --features="test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_link --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_link --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_link --tests --no-default-features --features="test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_link --tests --all-features -- -D warnings --no-deps

lightyear_messages:
    # `lightyear_messages` only works in `std`
    # cargo clippy -p lightyear_messages --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_messages --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_messages --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_messages --no-default-features --features="std test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_messages --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_messages --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_messages --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_messages --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_messages --tests --no-default-features --features="std test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_messages --tests --all-features -- -D warnings --no-deps

lightyear_netcode:
    # `lightyear_netcode` only works in `std`
    # `trace` only affects `server`
    # cargo clippy -p lightyear_netcode --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_netcode --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_netcode --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_netcode --no-default-features --features="std server trace" -- -D warnings --no-deps
    cargo clippy -p lightyear_netcode --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_netcode --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_netcode --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_netcode --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_netcode --tests --no-default-features --features="std server trace" -- -D warnings --no-deps
    cargo clippy -p lightyear_netcode --tests --all-features -- -D warnings --no-deps

lightyear_prediction:
    # `lightyear_prediction` only works in `std`
    # cargo clippy -p lightyear_prediction --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_prediction --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_prediction --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_prediction --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_prediction --tests --no-default-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_prediction --tests --no-default-features --features="std server" -- -D warnings --no-deps
    # cargo clippy -p lightyear_prediction --tests --no-default-features --features="metrics" -- -D warnings --no-deps
    # cargo clippy -p lightyear_prediction --tests --all-features -- -D warnings --no-deps

lightyear_replication:
    # `lightyear_replication` only works in `std`
    # cargo clippy -p lightyear_replication --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --no-default-features --features="std prediction" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --no-default-features --features="std interpolation" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --no-default-features --features="std trace" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --no-default-features --features="std test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_replication --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --tests --no-default-features --features="std prediction" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --tests --no-default-features --features="std interpolation" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --tests --no-default-features --features="std trace" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --tests --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --tests --no-default-features --features="std test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_replication --tests --all-features -- -D warnings --no-deps

lightyear_serde:
    cargo clippy -p lightyear_serde --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_serde --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_serde --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_serde --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_serde --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_serde --tests --all-features -- -D warnings --no-deps

lightyear_steam:
    cargo clippy -p lightyear_steam --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --no-default-features --features="client" -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --tests --no-default-features --features="client" -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --tests --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_steam --tests --all-features -- -D warnings --no-deps

lightyear_sync:
    # `lightyear_sync` only works in `std`
    # cargo clippy -p lightyear_sync --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_sync --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_sync --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_sync --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_sync --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_sync --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_sync --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_sync --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_sync --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_sync --tests --all-features -- -D warnings --no-deps

lightyear_tests:
    # `lightyear_tests` only works in `std`
    # cargo clippy -p lightyear_tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_tests --no-default-features --features="std" -- -D warnings --no-deps

lightyear_transport:
    # `lightyear_transport` only works in `std`
    # cargo clippy -p lightyear_transport --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --no-default-features --features="std trace" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --no-default-features --features="std test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --all-features -- -D warnings --no-deps
    # cargo clippy -p lightyear_transport --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --tests --no-default-features --features="std" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --tests --no-default-features --features="std client" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --tests --no-default-features --features="std server" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --tests --no-default-features --features="metrics" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --tests --no-default-features --features="std trace" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --tests --no-default-features --features="std test_utils" -- -D warnings --no-deps
    cargo clippy -p lightyear_transport --tests --all-features -- -D warnings --no-deps

lightyear_udp:
    cargo clippy -p lightyear_udp --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_udp --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_udp --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_udp --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_udp --tests --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_udp --tests --all-features -- -D warnings --no-deps

lightyear_utils:
    cargo clippy -p lightyear_utils --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_utils --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_utils --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_utils --tests --all-features -- -D warnings --no-deps

lightyear_webtransport:
    cargo clippy -p lightyear_webtransport --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --no-default-features --features="client" -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --no-default-features --features="self-signed" -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --no-default-features --features="dangerous-configuration" -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --tests --no-default-features -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --tests --no-default-features --features="client" -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --tests --no-default-features --features="server" -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --tests --no-default-features --features="self-signed" -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --tests --no-default-features --features="dangerous-configuration" -- -D warnings --no-deps
    cargo clippy -p lightyear_webtransport --tests --all-features -- -D warnings --no-deps

add_avian_symlinks:
    sed -i '' 's@path = "../lightyear_avian/src/lib.rs"@#path = "../lightyear_avian/src/lib.rs"@g' lightyear_avian2d/Cargo.toml
    sed -i '' 's@path = "../lightyear_avian/src/lib.rs"@#path = "../lightyear_avian/src/lib.rs"@g' lightyear_avian3d/Cargo.toml
    ln -s ../lightyear_avian/src lightyear_avian2d/src
    ln -s ../lightyear_avian/src lightyear_avian3d/src

remove_avian_symlinks:
    sed -i '' 's@#path = "../lightyear_avian/src/lib.rs"@path = "../lightyear_avian/src/lib.rs"@g' lightyear_avian2d/Cargo.toml
    sed -i '' 's@#path = "../lightyear_avian/src/lib.rs"@path = "../lightyear_avian/src/lib.rs"@g' lightyear_avian3d/Cargo.toml
    rm lightyear_avian2d/src
    rm lightyear_avian3d/src

release_dryrun_no_changelog:
    @just add_avian_symlinks
    cargo smart-release lightyear --allow-dirty -v -u --no-changelog --no-tag --no-push --dry-run-cargo-publish -b keep -d keep
    @just remove_avian_symlinks

release_dryrun:
    @just add_avian_symlinks
    cargo smart-release lightyear --allow-dirty -v -u --no-changelog-github-release --no-push --dry-run-cargo-publish -b keep -d keep
    @just remove_avian_symlinks

release_no_changelog:
    @just add_avian_symlinks
    cargo smart-release lightyear --allow-dirty -v -u --no-changelog --no-tag --no-push --execute -b keep -d keep --no-bump-on-demand
    @just remove_avian_symlinks

release:
    @just add_avian_symlinks
    cargo smart-release lightyear --allow-dirty -v -u --no-changelog-github-release --no-push --execute -b keep -d keep
    @just remove_avian_symlinks
