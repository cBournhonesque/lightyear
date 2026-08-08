#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

smoke_dir="$(mktemp -d "${TMPDIR:-/tmp}/lightyear-simple-box-p2p.XXXXXX")"
peer_0_log="$smoke_dir/peer-0.log"
peer_1_log="$smoke_dir/peer-1.log"
peer_0_debug="$smoke_dir/peer-0.jsonl"
peer_1_debug="$smoke_dir/peer-1.jsonl"
peer_0_state="$smoke_dir/peer-0-state.json"
peer_1_state="$smoke_dir/peer-1-state.json"

exit_tick="${LIGHTYEAR_SIMPLE_BOX_SMOKE_EXIT_TICK:-420}"
check_tick="${LIGHTYEAR_SIMPLE_BOX_SMOKE_CHECK_TICK:-380}"
base_port="${LIGHTYEAR_SIMPLE_BOX_SMOKE_PORT:-$((30000 + $$ % 10000))}"

if ((check_tick >= exit_tick)); then
    echo "check tick must be earlier than exit tick" >&2
    exit 1
fi

cargo build -p simple_box --no-default-features --features=p2p
target_dir="$(cargo metadata --no-deps --format-version 1 | jq -r .target_directory)"
binary="$target_dir/debug/simple_box"

pids=()
cleanup() {
    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT INT TERM

LIGHTYEAR_SIMPLE_BOX_AUTOMOVE=right \
LIGHTYEAR_SIMPLE_BOX_EXIT_AFTER_TICK="$exit_tick" \
LIGHTYEAR_DEBUG_FILE="$peer_0_debug" \
RUST_LOG=info,lightyear_debug=trace \
"$binary" --headless=true p2p --peer-id 0 --player-count 2 --base-port "$base_port" \
    >"$peer_0_log" 2>&1 &
pids+=("$!")

LIGHTYEAR_SIMPLE_BOX_AUTOMOVE=none \
LIGHTYEAR_SIMPLE_BOX_EXIT_AFTER_TICK="$exit_tick" \
LIGHTYEAR_DEBUG_FILE="$peer_1_debug" \
RUST_LOG=info,lightyear_debug=trace \
"$binary" --headless=true p2p --peer-id 1 --player-count 2 --base-port "$base_port" \
    >"$peer_1_log" 2>&1 &
pids+=("$!")

deadline=$((SECONDS + 20))
while kill -0 "${pids[0]}" 2>/dev/null || kill -0 "${pids[1]}" 2>/dev/null; do
    if ((SECONDS >= deadline)); then
        echo "simple-box P2P smoke test timed out; logs: $smoke_dir" >&2
        exit 1
    fi
    sleep 1
done

peer_0_status=0
peer_1_status=0
wait "${pids[0]}" || peer_0_status=$?
wait "${pids[1]}" || peer_1_status=$?
if ((peer_0_status != 0 || peer_1_status != 0)); then
    echo "simple-box P2P peer failed; logs: $smoke_dir" >&2
    exit 1
fi

if ! grep -q '"kind":"input_mismatch_rollback"' "$peer_1_debug"; then
    echo "peer 1 did not roll back for peer 0 input; logs: $smoke_dir" >&2
    exit 1
fi

extract_state() {
    local source="$1"
    local destination="$2"
    # App exit can interrupt the final buffered debug record. Parse complete JSONL
    # records individually so that an incomplete final line cannot hide the
    # earlier rollback and state samples under test.
    jq --null-input --raw-input --argjson tick "$check_tick" '
        [
            inputs
            | fromjson?
            | select(
                .category == "component"
                and .tick_id == $tick
                and (
                    (.component | endswith("PlayerId"))
                    or (.component | endswith("PlayerPosition"))
                )
            )
        ]
        | group_by(.entity)
        | map({
            player: ([.[] | select(.component | endswith("PlayerId")) | .value] | last),
            position: ([.[] | select(.component | endswith("PlayerPosition")) | .value] | last)
        })
        | map(select(.player != null and .position != null))
        | sort_by(.player | tojson)
    ' "$source" >"$destination"
}

extract_state "$peer_0_debug" "$peer_0_state"
extract_state "$peer_1_debug" "$peer_1_state"

if ! jq -e 'length == 2' "$peer_0_state" >/dev/null \
    || ! jq -e 'length == 2' "$peer_1_state" >/dev/null; then
    echo "did not capture both players at tick $check_tick; logs: $smoke_dir" >&2
    exit 1
fi
if ! diff -u "$peer_0_state" "$peer_1_state"; then
    echo "simple-box P2P peers diverged at tick $check_tick; logs: $smoke_dir" >&2
    exit 1
fi

trap - EXIT INT TERM
echo "simple-box P2P rollback and convergence passed at tick $check_tick"
echo "logs: $smoke_dir"
