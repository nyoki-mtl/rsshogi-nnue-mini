param(
    [Parameter(Mandatory = $true)]
    [string]$ModelPath
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$model = (Resolve-Path -LiteralPath $ModelPath -ErrorAction Stop).Path
if (-not (Test-Path -LiteralPath $model -PathType Leaf)) {
    throw "model file not found: $model"
}
$modelHash = (Get-FileHash -LiteralPath $model -Algorithm SHA256).Hash.ToLowerInvariant()
$targetDir = Join-Path $repoRoot 'target/embedded-avx2'
$source = Join-Path $targetDir 'release/rsshogi-nnue-mini.exe'
$dist = Join-Path $repoRoot 'dist'
$destination = Join-Path $dist 'rsshogi-nnue-mini-embedded-avx2.exe'

$previousFlags = $env:RUSTFLAGS
$previousPath = $env:RSSHOGI_EMBEDDED_RSNN_PATH
$previousHash = $env:RSSHOGI_EMBEDDED_RSNN_SHA256
try {
    $env:RUSTFLAGS = '-C target-feature=+avx2'
    $env:RSSHOGI_EMBEDDED_RSNN_PATH = $model
    $env:RSSHOGI_EMBEDDED_RSNN_SHA256 = $modelHash
    cargo build --release --locked --features embedded-rsnn --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed: $LASTEXITCODE"
    }
} finally {
    $env:RUSTFLAGS = $previousFlags
    $env:RSSHOGI_EMBEDDED_RSNN_PATH = $previousPath
    $env:RSSHOGI_EMBEDDED_RSNN_SHA256 = $previousHash
}

New-Item -ItemType Directory -Path $dist -Force | Out-Null
Copy-Item -LiteralPath $source -Destination $destination -Force
$binaryHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
@{
    binary = Split-Path -Leaf $destination
    binary_sha256 = $binaryHash
    embedded_rsnn_sha256 = $modelHash
    rustflags = '-C target-feature=+avx2'
    cargo_features = 'embedded-rsnn'
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $dist 'rsshogi-nnue-mini-embedded-avx2.json') -Encoding utf8
Write-Output $destination
