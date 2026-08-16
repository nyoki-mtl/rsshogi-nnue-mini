set shell := ["pwsh", "-NoProfile", "-Command"]

default:
	@just --list

build:
	cargo build

release:
	cargo build --release

release-tuning:
	cargo build --release --features tuning
	pwsh -NoProfile -File scripts/stage_engine_tuning.ps1

stage-engine: release
	pwsh -NoProfile -File scripts/stage_engine.ps1

test-avx2:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo test

release-avx2:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo build --release
	pwsh -NoProfile -File scripts/stage_engine_avx2.ps1

release-avx2-tuning:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo build --release --features tuning
	pwsh -NoProfile -File scripts/stage_engine_avx2_tuning.ps1

test:
	cargo test

fmt:
	cargo fmt --all

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings

book:
	mdbook build docs/book

