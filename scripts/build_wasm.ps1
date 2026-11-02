param(
    [string]$ModelPath,
    [string]$OutputDirectory = 'dist/wasm-sdk'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$targetDir = Join-Path $repoRoot 'target/wasm-sdk'
$destination = [System.IO.Path]::GetFullPath($OutputDirectory, $repoRoot)
$features = 'c-api'
$modelHash = $null
$previousFlags = $env:RUSTFLAGS
$previousPath = $env:RSSHOGI_EMBEDDED_RSNN_PATH
$previousHash = $env:RSSHOGI_EMBEDDED_RSNN_SHA256
Push-Location $repoRoot
try {
    # Keep this portable scalar Wasm build independent of native AVX2 flags.
    $env:RUSTFLAGS = ''
    if ($ModelPath) {
        $model = (Resolve-Path -LiteralPath $ModelPath).Path
        $modelHash = (Get-FileHash -LiteralPath $model -Algorithm SHA256).Hash.ToLowerInvariant()
        $env:RSSHOGI_EMBEDDED_RSNN_PATH = $model
        $env:RSSHOGI_EMBEDDED_RSNN_SHA256 = $modelHash
        $features += ',embedded-rsnn'
    }
    cargo rustc --lib --release --locked --crate-type cdylib --target wasm32-unknown-unknown --features $features --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed: $LASTEXITCODE" }
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    $artifact = Join-Path $destination 'rsshogi_nnue_mini.wasm'
    Copy-Item -LiteralPath (Join-Path $targetDir 'wasm32-unknown-unknown/release/rsshogi_nnue_mini.wasm') -Destination $artifact -Force
    foreach ($binding in @('mini.js', 'worker.js', 'package.json')) {
        Copy-Item -LiteralPath (Join-Path $repoRoot "bindings/wasm/$binding") -Destination $destination -Force
    }
    Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination $destination -Force
    $hashes = [ordered]@{}
    foreach ($name in @('rsshogi_nnue_mini.wasm', 'mini.js', 'worker.js', 'package.json', 'LICENSE')) {
        $hashes[$name] = (Get-FileHash -LiteralPath (Join-Path $destination $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $revision = (& git rev-parse HEAD)
    if ($LASTEXITCODE -ne 0) { throw 'git rev-parse failed' }
    $dirty = [bool](& git status --porcelain)
    if ($LASTEXITCODE -ne 0) { throw 'git status failed' }
    $version = [regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    @{
        api_version = 1
        package_version = $version
        source_revision = $revision
        source_revision_scope = 'build checkout; public snapshot commit may differ'
        public_source_tag = 'v' + ([regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value)
        source_dirty = $dirty
        artifact = 'rsshogi_nnue_mini.wasm'
        artifact_sha256 = (Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
        embedded_rsnn_sha256 = $modelHash
        target = 'wasm32-unknown-unknown'
        cargo_features = $features
        threads = 1
        artifacts_sha256 = $hashes
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $destination 'manifest.json') -Encoding utf8
    Write-Output $destination
} finally {
    Pop-Location
    $env:RUSTFLAGS = $previousFlags
    $env:RSSHOGI_EMBEDDED_RSNN_PATH = $previousPath
    $env:RSSHOGI_EMBEDDED_RSNN_SHA256 = $previousHash
}
