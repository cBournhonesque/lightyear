default: format taplo typos clippy test

# Local CI
format:
    cargo fmt

taplo:
    taplo fmt

typos:
    typos -w

doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items --keep-going --all-features --features="lightyear_avian2d/f32 lightyear_avian3d/f32"

# Keep local Clippy aligned with CI. The excluded GUI examples are linted in
# separate feature graphs so Avian 2D and 3D are not unified.
clippy:
    cargo clippy --features=lightyear_core/not_mock,avian3d/f32 --workspace --exclude=compiletime --exclude=avian_3d --exclude=launcher --exclude=delta_compression --no-deps -- -D warnings -A clippy::needless_lifetimes
    cargo clippy -p avian_3d --all-features --no-deps -- -D warnings
    cargo clippy -p launcher --all-features --no-deps -- -D warnings

# Run two conditioned simple-box peers and verify remote prediction rollback plus convergence.
simple_box_p2p_smoke:
    bash examples/simple_box/p2p_smoke.sh

# jq filters shared by the example/demo build recipe.
[private]
_example_demo_pkgs_filter := '.packages[] | select((.manifest_path | test("/(examples|demos)/")) and (.manifest_path | test("/examples/common/") | not) and (.manifest_path | test("/examples/launcher/") | not) and (.name != "simple_setup")) | .name'
[private]
_example_demo_p2p_pkgs_filter := '.packages[] | select((.manifest_path | test("/(examples|demos)/")) and (.manifest_path | test("/examples/common/") | not) and (.features | has("p2p"))) | .name'
# simple_setup is excluded from explicit feature builds because it has its own feature setup.

# Build all examples/demos with the requested role features. Client/server add
# the default netcode + WebTransport stack, and client/P2P add the GUI. When P2P
# is requested, only examples that expose P2P are selected, and the Avian 2D/3D
# examples are built separately to avoid feature unification.
#
# Usage:
#   just build_examples
#   just build_examples features=client,server
#   just build_examples features=client headless=true
#   just build_examples names=avian_3d
#   just build_examples names=avian_2d,avian_3d features=client headless=true
#   just build_examples features=client,server,p2p
#   just build_examples release=true features=server headless=false
#
# Args:
#   release=true|false   Defaults to false.
#   headless=true|false  Removes/adds gui. Defaults based on the requested role.
# names=NAME,... selects packages; features=FEATURE,... defaults to client,server.
build_examples *args:
    #!/usr/bin/env bash
    set -euo pipefail
    usage='usage: just build_examples [release=true|false] [headless=true|false] [features=FEATURE,...] [names=NAME,...]'
    release=false
    headless=""
    names=""
    requested_features="client,server"
    for arg in {{ args }}; do
        case "$arg" in
            release=true|release=false)
                release="${arg#release=}"
                ;;
            headless=true|headless=false)
                headless="${arg#headless=}"
                ;;
            features=*)
                requested_features="${arg#features=}"
                if [ -z "$requested_features" ]; then
                    echo "features must not be empty" >&2
                    exit 2
                fi
                ;;
            names=*)
                names="${arg#names=}"
                if [ -z "$names" ]; then
                    echo "names must not be empty" >&2
                    exit 2
                fi
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

    cargo_features="$requested_features"
    has_feature() {
        case ",$cargo_features," in
            *,"$1",*) return 0 ;;
            *) return 1 ;;
        esac
    }
    add_feature() {
        if ! has_feature "$1"; then
            cargo_features="$cargo_features,$1"
        fi
    }
    remove_feature() {
        local padded_features=",$cargo_features,"
        padded_features="${padded_features//,$1,/,}"
        cargo_features="${padded_features#,}"
        cargo_features="${cargo_features%,}"
    }
    if has_feature client || has_feature server; then
        add_feature netcode
        add_feature webtransport
    fi
    if [ "$headless" = true ]; then
        remove_feature gui
    elif [ "$headless" = false ]; then
        add_feature gui
    elif has_feature client || has_feature p2p; then
        add_feature gui
    fi

    cargo_build=(cargo build -j 1)
    if [ "$release" = true ]; then
        cargo_build+=(--release)
    fi

    p2p=false
    case ",$cargo_features," in
        *,p2p,*) p2p=true ;;
    esac

    workspace_metadata="$(cargo metadata --no-deps --format-version 1)"
    selected_packages=()
    if [ -n "$names" ]; then
        IFS=',' read -r -a requested_names <<< "$names"
        IFS=',' read -r -a resolved_features <<< "$cargo_features"
        for pkg in "${requested_names[@]}"; do
            if [ -z "$pkg" ]; then
                echo "names must not contain an empty package name" >&2
                exit 2
            fi
            if ! jq -e --arg pkg "$pkg" '
                any(.packages[];
                    .name == $pkg
                    and (.manifest_path | test("/(examples|demos)/"))
                    and (.manifest_path | test("/examples/common/") | not)
                )
            ' <<< "$workspace_metadata" >/dev/null; then
                echo "unknown example/demo package: $pkg" >&2
                exit 2
            fi
            for feature in "${resolved_features[@]}"; do
                if ! jq -e --arg pkg "$pkg" --arg feature "$feature" '
                    any(.packages[]; .name == $pkg and (.features | has($feature)))
                ' <<< "$workspace_metadata" >/dev/null; then
                    echo "example/demo package '$pkg' does not support feature '$feature'" >&2
                    exit 2
                fi
            done
            selected_packages+=("$pkg")
        done
    else
        if [ "$p2p" = true ]; then
            pkgs_filter='{{ _example_demo_p2p_pkgs_filter }}'
        else
            pkgs_filter='{{ _example_demo_pkgs_filter }}'
        fi
        while IFS= read -r pkg; do
            selected_packages+=("$pkg")
        done < <(jq -r "$pkgs_filter" <<< "$workspace_metadata" | sort)
    fi

    echo "Building examples: release=$release headless=${headless:-auto} requested=$requested_features features=$cargo_features"
    echo "Selected packages: ${selected_packages[*]}"
    package_args=()
    non_avian_count=0
    build_avian_2d=false
    build_avian_3d=false
    for pkg in "${selected_packages[@]}"; do
        if [ "$p2p" = true ] && [ "$pkg" = avian_2d ]; then
            build_avian_2d=true
        elif [ "$p2p" = true ] && [ "$pkg" = avian_3d ]; then
            build_avian_3d=true
        else
            package_args+=(-p "$pkg")
            non_avian_count=$((non_avian_count + 1))
        fi
    done
    if [ "$non_avian_count" -gt 0 ]; then
        "${cargo_build[@]}" --no-default-features --features="$cargo_features" "${package_args[@]}"
    fi
    if [ "$build_avian_2d" = true ]; then
        "${cargo_build[@]}" --no-default-features --features="$cargo_features" -p avian_2d
    fi
    if [ "$build_avian_3d" = true ]; then
        "${cargo_build[@]}" --no-default-features --features="$cargo_features" -p avian_3d
    fi

test:
    # Can´t do --workspace because of feature unification with the packages in examples.
    # You can't use `--all-features` because of conflict between `avian2d` and `avian3d`.
    cargo test -p lightyear --no-default-features --features="std client server replication \
    interpolation trace metrics netcode webtransport webtransport_self_signed webtransport_dangerous_configuration \
    input_native leafwing input_bei avian2d lightyear_avian2d/f32 udp websocket crossbeam steam"
    cargo test -p lightyear --no-default-features --features="std client server replication \
    interpolation trace metrics netcode webtransport webtransport_self_signed webtransport_dangerous_configuration \
    input_native leafwing input_bei avian3d lightyear_avian3d/f32 udp websocket crossbeam steam"
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
    cargo release --no-publish --no-tag --no-push --workspace --config .release.toml "{{ version }}"
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
    cargo release --execute --no-publish --no-tag --no-push --workspace --config .release.toml "{{ version }}"
    just add_avian_symlinks
    pkgs=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.publish != []) | "-p " + .name' | tr "\n" " ")
    cargo publish --allow-dirty -j 4 $pkgs
