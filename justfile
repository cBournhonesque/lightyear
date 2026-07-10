default: format taplo typos clippy clippy_examples test

# Local CI
format:
    cargo fmt

taplo:
    taplo fmt

typos:
    typos -w

doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --keep-going --all-features --features="lightyear_avian2d/f32 lightyear_avian3d/f32"

clippy: lightyear lightyear_aeronet lightyear_avian lightyear_connection lightyear_core \
    lightyear_crossbeam lightyear_frame_interpolation lightyear_inputs lightyear_interpolation \
    lightyear_link lightyear_messages lightyear_netcode lightyear_prediction lightyear_replication \
    lightyear_serde lightyear_sync lightyear_tests lightyear_transport lightyear_udp lightyear_utils lightyear_webtransport
    # You can't use `--all-features` because of conflict between `avian2d` and `avian3d`.
    # cargo clippy --workspace --examples --tests --all-features -- -D warnings

clippy_examples:
    cargo clippy -p spaceships --all-features -- -D warnings --no-deps
    cargo clippy -p auth --all-features -- -D warnings --no-deps
    cargo clippy -p avian_3d --all-features -- -D warnings --no-deps
    cargo clippy -p avian_2d --all-features -- -D warnings --no-deps
    cargo clippy -p bevy_enhanced_inputs --all-features -- -D warnings --no-deps
    cargo clippy -p lightyear_examples_common --all-features -- -D warnings --no-deps
    # cargo clippy -p delta_compression --all-features -- -D warnings --no-deps
    cargo clippy -p fps --all-features -- -D warnings --no-deps
    cargo clippy -p launcher --all-features -- -D warnings --no-deps
    cargo clippy -p lobby --all-features -- -D warnings --no-deps
    cargo clippy -p network_visibility --all-features -- -D warnings --no-deps
    cargo clippy -p priority --all-features -- -D warnings --no-deps
    cargo clippy -p replication_groups --all-features -- -D warnings --no-deps
    cargo clippy -p simple_box --all-features -- -D warnings --no-deps
    # cargo clippy -p simple_setup --all-features -- -D warnings --no-deps

# jq filters shared by the example/demo build recipe.
_example_demo_non_projectiles_pkgs_filter := '.packages[] | select((.manifest_path | test("/(examples|demos)/")) and (.manifest_path | test("/examples/common/") | not) and (.manifest_path | test("/examples/launcher/") | not) and (.name != "simple_setup") and (.name != "projectiles")) | .name'
# simple_setup is excluded from explicit feature builds because it has no client/server/gui feature gates.
_wasm_example_pkgs_filter := '.packages[] | select(.metadata.bevy_cli.web != null) | .name'

# Build all examples/demos.
#
# Usage:
#   just build_examples
#   just build_examples features=server
#   just build_examples features=client headless=true
#   just build_examples release=true features=both
#
# Args:
#   release=true|false   Defaults to false.
#   headless=true|false  Defaults to true for features=server, false otherwise.
#   features=server|client|both  Defaults to both.
build_examples *args:
    #!/usr/bin/env bash
    set -euo pipefail
    usage='usage: just build_examples [release=true|false] [headless=true|false] [features=server|client|both]'
    release=false
    headless=""
    feature_mode=both
    for arg in {{args}}; do
        case "$arg" in
            release=true|release=false)
                release="${arg#release=}"
                ;;
            headless=true|headless=false)
                headless="${arg#headless=}"
                ;;
            features=server|features=client|features=both)
                feature_mode="${arg#features=}"
                ;;
            -h|--help|help)
                echo "$usage"
                exit 0
                ;;
            *)
                echo "$usage" >&2
                echo "unknown argument: $arg" >&2
                exit 2
                ;;
        esac
    done
    if [ -z "$headless" ]; then
        if [ "$feature_mode" = server ]; then
            headless=true
        else
            headless=false
        fi
    fi

    cargo_build=(cargo build -j 1)
    release_suffix=""
    if [ "$release" = true ]; then
        cargo_build+=(--release)
        release_suffix="-release"
    fi

    target_dir=""
    if [ "$headless" = true ]; then
        case "$feature_mode" in
            both) target_dir="target/headless${release_suffix}" ;;
            client) target_dir="target/headless-client${release_suffix}" ;;
            server) target_dir="target/headless-server${release_suffix}" ;;
        esac
    elif [ "$feature_mode" = server ]; then
        target_dir="target/server-only${release_suffix}"
    fi
    if [ -n "$target_dir" ]; then
        cargo_build+=(--target-dir "$target_dir")
    fi

    case "$feature_mode:$headless" in
        both:true) cargo_features="client,server,netcode,webtransport" ;;
        client:true) cargo_features="client,netcode,webtransport" ;;
        server:true) cargo_features="server,netcode,webtransport" ;;
        both:false) cargo_features="client,gui,server,netcode,webtransport" ;;
        client:false) cargo_features="client,gui,netcode,webtransport" ;;
        server:false) cargo_features="server,gui,netcode,webtransport" ;;
    esac

    if [ "$headless" = true ]; then
        projectiles_features="client,server,netcode,webtransport"
    else
        projectiles_features="client,gui,server,netcode,webtransport"
    fi

    echo "Building examples: release=$release headless=$headless features=$feature_mode cargo_features=$cargo_features"
    pkgs=$(cargo metadata --no-deps --format-version 1 | jq -r '{{ _example_demo_non_projectiles_pkgs_filter }}' | sort | sed 's/^/-p /' | tr "\n" " ")
    "${cargo_build[@]}" --no-default-features --features="$cargo_features" $pkgs
    "${cargo_build[@]}" --no-default-features --features="$projectiles_features" -p projectiles

# Build all examples/demos with a declared WASM web target.
#
# Usage:
#   just build_examples_wasm
#   just build_examples_wasm release=true
#
# Args:
#   release=true|false   Defaults to false.
build_examples_wasm *args:
    #!/usr/bin/env bash
    set -euo pipefail
    usage='usage: just build_examples_wasm [release=true|false]'
    release=false
    for arg in {{args}}; do
        case "$arg" in
            release=true|release=false)
                release="${arg#release=}"
                ;;
            -h|--help|help)
                echo "$usage"
                exit 0
                ;;
            *)
                echo "$usage" >&2
                echo "unknown argument: $arg" >&2
                exit 2
                ;;
        esac
    done

    cargo_build=(cargo build -j 1 --target wasm32-unknown-unknown)
    if [ "$release" = true ]; then
        cargo_build+=(--release)
    else
        export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
    fi

    cargo_features="client,netcode,webtransport"
    pkgs=$(cargo metadata --no-deps --format-version 1 | jq -r '{{ _wasm_example_pkgs_filter }}' | sort | sed 's/^/-p /' | tr "\n" " ")
    if [ -z "$pkgs" ]; then
        echo "no WASM example packages found" >&2
        exit 1
    fi

    echo "Building WASM examples: release=$release cargo_features=$cargo_features"
    "${cargo_build[@]}" --no-default-features --features="$cargo_features" $pkgs

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
    #!/usr/bin/env bash
    set -euo pipefail
    for crate in crates/integration/avian2d crates/integration/avian3d; do
        src="$crate/src"
        if [ -e "$src" ] && [ ! -L "$src" ]; then
            echo "$src exists and is not a symlink" >&2
            exit 1
        fi
        rm -f "$src"
        perl -0pi -e 's@(?m)^path = "\.\./avian/src/lib\.rs"$@#path = "../avian/src/lib.rs"@' "$crate/Cargo.toml"
        ln -s ../avian/src "$src"
    done

remove_avian_symlinks:
    #!/usr/bin/env bash
    set -euo pipefail
    for crate in crates/integration/avian2d crates/integration/avian3d; do
        src="$crate/src"
        if [ -L "$src" ]; then
            rm "$src"
        elif [ -e "$src" ]; then
            echo "$src exists and is not a symlink; leaving it in place" >&2
        fi
        perl -0pi -e 's@(?m)^#path = "\.\./avian/src/lib\.rs"$@path = "../avian/src/lib.rs"@' "$crate/Cargo.toml"
    done

release_dryrun version:
    #!/usr/bin/env bash
    set -euo pipefail
    cleanup() {
        status=$?
        just remove_avian_symlinks >/dev/null 2>&1 || true
        exit "$status"
    }
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    cargo release --no-publish --no-tag --no-push --workspace --config .release.toml "{{version}}"
    just add_avian_symlinks
    pkgs=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.publish != []) | "-p " + .name' | tr "\n" " ")
    cargo package --allow-dirty -j 4 $pkgs

release version:
    #!/usr/bin/env bash
    set -euo pipefail
    cleanup() {
        status=$?
        just remove_avian_symlinks >/dev/null 2>&1 || true
        exit "$status"
    }
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    cargo release --execute --no-publish --no-tag --no-push --workspace --config .release.toml "{{version}}"
    just add_avian_symlinks
    pkgs=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.publish != []) | "-p " + .name' | tr "\n" " ")
    cargo publish --allow-dirty -j 4 $pkgs
