#!/bin/bash -e
# Run shellcheck on this after modifications.
# Assuming this script is kept in the examples dir
cd "$(dirname "$0")"
BASE_DIR=$(builtin cd .. ; pwd)
EXAMPLES_DIR="$BASE_DIR/examples"
HTDOCS_DIR="$BASE_DIR/htdocs"
CARGO_RELEASE_DIR="$BASE_DIR/target/wasm32-unknown-unknown/release"
export RUSTFLAGS=--cfg=web_sys_unstable_apis
export CARGO_BUILD_TARGET=wasm32-unknown-unknown

if [[ ! -z "$RUNNER_OS" ]]; then
    # an attempt to get the github runner to not kill us due to resource usage
    echo "Running on github runner, setting CARGO_BUILD_JOBS lower"
    export CARGO_BUILD_JOBS=2
fi

# load "example_list" variable from .env file, shared with Dockerfile.server
set -a
source ./example_list.env
set +a

if [[ -z "$example_list" ]]; then
    echo "example_list is not set" >&2
    exit 1
fi
echo "example_list=$example_list"

# Find clang
if command -v brew >/dev/null 2>&1; then
    # if brew command exists, assume macos on a github runner
    TARGET_CC="$(brew --prefix llvm@15)/bin/clang"
fi

if [[ ! -x "$TARGET_CC" ]] ; then
    # ssume linux standard path
    TARGET_CC="/usr/bin/clang"
fi

if [[ ! -x "$TARGET_CC" ]]; then
    echo "Clang not found at '$TARGET_CC'. Please install it or specify its path manually." >&2
    exit 1
fi
export TARGET_CC
echo "Using clang at $TARGET_CC"

# we want to compile all examples in the list with one cargo command, which is much faster,
# because it only builds the shared deps once.
# make a string of "-p example1 -p example2" etc to pass to cargo.
p_args="-p $(echo $example_list | sed 's/,/ -p /g')"

echo "Building examples: $p_args"
cargo build --release --no-default-features -F bevygap_client $p_args

echo "Running wasm-bindgen for each example"
for example in $(echo $example_list | tr ',' ' ') ;
do
	(
	outdir="$HTDOCS_DIR/$example"
	mkdir -p "$outdir"
	cd "$EXAMPLES_DIR/$example"
	wasm-bindgen --no-typescript --target web --out-dir "$outdir" --out-name "$example" "$CARGO_RELEASE_DIR/$example.wasm"
	sed -e "s/{{name}}/$example/g" "$EXAMPLES_DIR/common/www/index.html" > "$outdir/index.html"
	)
done

# TODO write a proper template for this:
echo "<html><head><title>Lightyear Examples Menu</title></head><body>" > "$HTDOCS_DIR/index.html"

for example in $(echo $example_list | tr ',' ' ') ; do
	echo "<ul><a href=\"$example/\">$example</a></ul>" >> "$HTDOCS_DIR/index.html"
done

echo "</body></html>" >> "$HTDOCS_DIR/index.html"

echo "$HTDOCS_DIR is ready to ship"
