$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot 'target/release/rsshogi-nnue-mini.exe'
$dist = Join-Path $repoRoot 'dist'
$destination = Join-Path $dist 'rsshogi-nnue-mini.exe'

if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "release binary not found: $source; run cargo build --release first"
}
if (-not (Test-Path -LiteralPath $dist -PathType Container)) {
    New-Item -ItemType Directory -Path $dist | Out-Null
}
Copy-Item -LiteralPath $source -Destination $destination -Force
Write-Output $destination
