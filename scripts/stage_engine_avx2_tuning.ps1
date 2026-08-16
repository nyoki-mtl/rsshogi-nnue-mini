$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot 'target/release/rsshogi-nnue-mini.exe'
$dist = Join-Path $repoRoot 'dist'
$destination = Join-Path $dist 'rsshogi-nnue-mini-tuning-avx2.exe'

if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "AVX2 tuning release binary not found: $source; run cargo build --release --features tuning first"
}
if (-not (Test-Path -LiteralPath $dist -PathType Container)) {
    New-Item -ItemType Directory -Path $dist | Out-Null
}
Copy-Item -LiteralPath $source -Destination $destination -Force
Write-Output $destination
