set shell := ["pwsh", "-NoProfile", "-Command"]

default:
	@just --list

build:
	cargo build

release:
	cargo build --release
	pwsh -NoProfile -File scripts/stage_engine.ps1

release-tuning:
	cargo build --release --features tuning
	pwsh -NoProfile -File scripts/stage_engine.ps1 -Variant tuning

stage-engine: release

test-avx2:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo test

release-avx2:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo build --release
	pwsh -NoProfile -File scripts/stage_engine.ps1 -Variant avx2

release-avx2-embedded model_path:
	pwsh -NoProfile -File scripts/build_embedded_avx2.ps1 -ModelPath "{{model_path}}"

release-avx2-tuning:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo build --release --features tuning
	pwsh -NoProfile -File scripts/stage_engine.ps1 -Variant avx2-tuning

test:
	cargo test --workspace

fmt:
	cargo fmt --all

lint:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

book:
	mdbook build docs/book

check-mobile:
	cargo clippy --target aarch64-apple-ios --all-targets -- -D warnings
	cargo build --release --lib --target aarch64-apple-ios
	cargo clippy --target aarch64-linux-android --all-targets -- -D warnings
	cargo build --release --lib --target aarch64-linux-android
	$env:RUSTFLAGS = "-C target-feature=+dotprod"; cargo clippy --target aarch64-apple-ios --all-targets -- -D warnings
	$env:RUSTFLAGS = "-C target-feature=+dotprod"; cargo build --release --lib --target aarch64-apple-ios
	$env:RUSTFLAGS = "-C target-feature=+dotprod"; cargo clippy --target aarch64-linux-android --all-targets -- -D warnings
	$env:RUSTFLAGS = "-C target-feature=+dotprod"; cargo build --release --lib --target aarch64-linux-android

