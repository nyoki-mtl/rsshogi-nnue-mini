param(
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')]
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$OutputDirectory = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $IsWindows) { throw 'build_sdk.ps1 requires Windows and the MSVC toolchain' }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repoRoot "dist/sdk/$Target" }
$features = @('c-api')
$modelHash = $null
if ($env:RSSHOGI_EMBEDDED_RSNN_PATH) {
    $modelHash = (Get-FileHash -LiteralPath $env:RSSHOGI_EMBEDDED_RSNN_PATH -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($modelHash -cne $env:RSSHOGI_EMBEDDED_RSNN_SHA256) {
        throw 'RSSHOGI_EMBEDDED_RSNN_SHA256 must match the model SHA-256 (lowercase)'
    }
    $features += 'embedded-rsnn'
} elseif ($env:RSSHOGI_EMBEDDED_RSNN_SHA256) {
    throw 'RSSHOGI_EMBEDDED_RSNN_PATH is required with RSSHOGI_EMBEDDED_RSNN_SHA256'
}

$previousFlags = $env:RUSTFLAGS
Push-Location $repoRoot
try {
    # Match the Windows USI release: x86-64 requires AVX2; ARM64 uses NEON.
    $env:RUSTFLAGS = if ($Target -eq 'x86_64-pc-windows-msvc') { '-C target-feature=+avx2' } else { '' }
    $revision = (& git rev-parse HEAD)
    if ($LASTEXITCODE -ne 0) { throw 'git rev-parse failed' }
    $dirty = [bool](& git status --porcelain)
    if ($LASTEXITCODE -ne 0) { throw 'git status failed' }
    $targetDir = Join-Path $repoRoot 'target/sdk-native'
    & cargo rustc --locked --release --lib --crate-type staticlib,cdylib --features ($features -join ',') --target $Target --target-dir $targetDir
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed: $LASTEXITCODE" }
    $buildDir = Join-Path $targetDir "$Target/release"
    # Rust MSVC emits distinct import and static libraries. Ship both explicitly.
    $files = @('rsshogi_nnue_mini.dll', 'rsshogi_nnue_mini.dll.lib', 'rsshogi_nnue_mini.lib')
    foreach ($file in $files) {
        if (-not (Test-Path -LiteralPath (Join-Path $buildDir $file) -PathType Leaf)) {
            throw "missing build artifact: $file"
        }
    }
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    foreach ($file in $files) {
        Copy-Item -LiteralPath (Join-Path $buildDir $file) -Destination $OutputDirectory -Force
    }
    Copy-Item -LiteralPath 'include/rsshogi_nnue_mini.h' -Destination $OutputDirectory -Force
    Copy-Item -LiteralPath 'LICENSE' -Destination $OutputDirectory -Force
    $hashes = [ordered]@{}
    foreach ($file in ($files + @('rsshogi_nnue_mini.h', 'LICENSE'))) {
        $hashes[$file] = (Get-FileHash -LiteralPath (Join-Path $OutputDirectory $file) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    [ordered]@{
        source_revision = $revision
        source_revision_scope = 'build checkout; public snapshot commit may differ'
        public_source_tag = 'v' + ([regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value)
        source_dirty = $dirty
        build_target = $Target
        cargo_features = $features
        embedded_rsnn_sha256 = $modelHash
        rustflags = $env:RUSTFLAGS
        simd_backend = if ($Target -eq 'x86_64-pc-windows-msvc') { 'avx2' } else { 'neon (runtime dotprod detection)' }
        artifacts_sha256 = $hashes
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'sdk.json') -Encoding utf8
    Write-Output $OutputDirectory
} finally {
    $env:RUSTFLAGS = $previousFlags
    Pop-Location
}
