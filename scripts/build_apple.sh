#!/usr/bin/env bash
# Build locally on macOS with Xcode and the requested Rust targets installed.
# Optional embedded model: set both RSSHOGI_EMBEDDED_RSNN_PATH and
# RSSHOGI_EMBEDDED_RSNN_SHA256. No separate model file is copied to the SDK.
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"
if [[ "$(uname -s)" != Darwin ]]; then
    echo 'build_apple.sh requires macOS and Xcode' >&2
    exit 1
fi
output="${1:-$repo_root/dist/apple}"
mkdir -p "$output"
output="$(cd "$output" && pwd)"
if [[ -e "$output/MiniEngine.xcframework" ]]; then
    echo "output already exists: $output/MiniEngine.xcframework (choose a new output directory)" >&2
    exit 1
fi
features=c-api
model_hash=''
if [[ -n "${RSSHOGI_EMBEDDED_RSNN_PATH:-}" ]]; then
    model_hash="$(shasum -a 256 "$RSSHOGI_EMBEDDED_RSNN_PATH" | awk '{print $1}')"
    if [[ "$model_hash" != "${RSSHOGI_EMBEDDED_RSNN_SHA256:-}" ]]; then
        echo 'RSSHOGI_EMBEDDED_RSNN_SHA256 must match the model SHA-256 (lowercase)' >&2
        exit 1
    fi
    features+=,embedded-rsnn
elif [[ -n "${RSSHOGI_EMBEDDED_RSNN_SHA256:-}" ]]; then
    echo 'RSSHOGI_EMBEDDED_RSNN_PATH is required with RSSHOGI_EMBEDDED_RSNN_SHA256' >&2
    exit 1
fi
target_dir="$repo_root/target/sdk-apple"
headers="$target_dir/headers"
mkdir -p "$headers"
cp include/rsshogi_nnue_mini.h "$headers/"
cp bindings/apple/module.modulemap "$headers/"
targets=(aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin)
# Opt in to Intel slices without combining device and simulator archives.
if [[ "${MINI_APPLE_INTEL:-0}" == 1 ]]; then
    targets+=(x86_64-apple-ios x86_64-apple-darwin)
fi
for target in "${targets[@]}"; do
    cargo rustc --locked --release --lib --crate-type staticlib --features "$features" \
        --target "$target" --target-dir "$target_dir"
done
sim_library="$target_dir/aarch64-apple-ios-sim/release/librsshogi_nnue_mini.a"
mac_library="$target_dir/aarch64-apple-darwin/release/librsshogi_nnue_mini.a"
if [[ "${MINI_APPLE_INTEL:-0}" == 1 ]]; then
    xcrun lipo -create "$sim_library" "$target_dir/x86_64-apple-ios/release/librsshogi_nnue_mini.a" -output "$target_dir/simulator.a"
    xcrun lipo -create "$mac_library" "$target_dir/x86_64-apple-darwin/release/librsshogi_nnue_mini.a" -output "$target_dir/macos.a"
    sim_library="$target_dir/simulator.a"
    mac_library="$target_dir/macos.a"
fi
xcodebuild -create-xcframework \
    -library "$target_dir/aarch64-apple-ios/release/librsshogi_nnue_mini.a" -headers "$headers" \
    -library "$sim_library" -headers "$headers" \
    -library "$mac_library" -headers "$headers" \
    -output "$output/MiniEngine.xcframework"
cp bindings/apple/MiniEngine.swift "$output/"
cp LICENSE "$output/"
# Type-check the distributed wrapper against the same C module and host SDK.
xcrun swiftc -swift-version 6 -typecheck -I "$headers" bindings/apple/MiniEngine.swift
host_target="$(rustc -vV | sed -n 's/^host: //p')"
cargo rustc --locked --release --lib --crate-type cdylib --features "$features" \
    --target "$host_target" --target-dir "$target_dir"
host_lib="$target_dir/$host_target/release"
xcrun swiftc -swift-version 6 -I "$headers" -L "$host_lib" -lrsshogi_nnue_mini \
    -Xlinker -rpath -Xlinker "$host_lib" \
    bindings/apple/MiniEngine.swift scripts/check_apple.swift -o "$target_dir/check-apple"
"$target_dir/check-apple"
cargo build --locked --release --bin rsshogi-nnue-mini --features "$features" \
    --target aarch64-apple-darwin --target-dir "$target_dir"
cp "$target_dir/aarch64-apple-darwin/release/rsshogi-nnue-mini" "$output/rsshogi-nnue-mini-macos-arm64"
export MINI_SDK_OUTPUT="$output" MINI_SDK_FEATURES="$features" MINI_SDK_MODEL_HASH="$model_hash"
export MINI_SDK_TARGETS="${targets[*]}"
python3 - <<'PY'
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess

output = Path(os.environ['MINI_SDK_OUTPUT'])
metadata = {
    'source_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'source_revision_scope': 'build checkout; public snapshot commit may differ',
    'public_source_tag': 'v' + re.search(r'^version\s*=\s*"([^"]+)"', Path('Cargo.toml').read_text(), re.M)[1],
    'source_dirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], text=True).strip()),
    'build_targets': os.environ['MINI_SDK_TARGETS'].split(),
    'cargo_features': os.environ['MINI_SDK_FEATURES'].split(','),
    'embedded_rsnn_sha256': os.environ['MINI_SDK_MODEL_HASH'] or None,
    'rustflags': os.environ.get('RUSTFLAGS'),
    'validation': {'swift_host_smoke': True, 'ios_device_runtime': False, 'ios_simulator_runtime': False},
    'artifacts_sha256': {
        path.relative_to(output).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(output.rglob('*')) if path.is_file() and path.name != 'sdk.json'
    },
}
(output / 'sdk.json').write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
PY
echo "$output/MiniEngine.xcframework"
