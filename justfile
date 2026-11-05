set shell := ["pwsh", "-NoProfile", "-Command"]

default:
	@just --list

_stage profile binary:
	New-Item -ItemType Directory -Path dist -Force | Out-Null
	Copy-Item -LiteralPath "target/{{profile}}/rsshogi-nnue-mini.exe" -Destination "dist/{{binary}}" -Force
	Write-Output (Resolve-Path -LiteralPath "dist/{{binary}}").Path

build:
	cargo build

release:
	cargo build --release
	@just _stage release rsshogi-nnue-mini.exe

release-tuning:
	cargo build --release --features tuning
	@just _stage release rsshogi-nnue-mini-tuning.exe

stage-engine: release

test-avx2:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo test

release-avx2:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo build --release
	@just _stage release rsshogi-nnue-mini-avx2.exe

release-avx2-embedded model_path:
	pwsh -NoProfile -File scripts/build_embedded_avx2.ps1 -ModelPath "{{model_path}}"

release-avx2-tuning:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo build --release --features tuning
	@just _stage release rsshogi-nnue-mini-tuning-avx2.exe

production-avx2:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo build --profile production
	@just _stage production rsshogi-nnue-mini-avx2.exe

production-avx2-tuning:
	$env:RUSTFLAGS = "-C target-feature=+avx2"; cargo build --profile production --features tuning
	@just _stage production rsshogi-nnue-mini-tuning-avx2.exe

test:
	cargo test

fmt:
	cargo fmt --all

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings

book:
	mdbook build docs/book

check-mobile:
	cargo clippy --target aarch64-apple-ios --lib --features c-api -- -D warnings
	cargo clippy --target aarch64-linux-android --lib --features c-api -- -D warnings
	$env:RUSTFLAGS = "-C target-feature=+dotprod"; cargo clippy --target aarch64-apple-ios --lib --features c-api -- -D warnings
	$env:RUSTFLAGS = "-C target-feature=+dotprod"; cargo clippy --target aarch64-linux-android --lib --features c-api -- -D warnings

sdk-wasm:
	pwsh -NoProfile -File scripts/build_wasm.ps1

sdk-wasm-embedded model_path:
	pwsh -NoProfile -File scripts/build_wasm.ps1 -ModelPath "{{model_path}}"

