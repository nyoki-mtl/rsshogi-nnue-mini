# 直前のreleaseビルドを、ShogiArenaの設定が参照する固定の配置先へ複製する。
#
# ビルドの種類ごとに配置先の名前を変える。cargoは同じ`target/release`へ出力するため、
# 名前を分けておかないとvariantを取り違える。
param(
    [ValidateSet('release', 'avx2', 'tuning', 'avx2-tuning')]
    [string]$Variant = 'release'
)

$ErrorActionPreference = 'Stop'

$name = switch ($Variant) {
    'release' { 'rsshogi-nnue-mini.exe' }
    'avx2' { 'rsshogi-nnue-mini-avx2.exe' }
    'tuning' { 'rsshogi-nnue-mini-tuning.exe' }
    'avx2-tuning' { 'rsshogi-nnue-mini-tuning-avx2.exe' }
}
$build = if ($Variant -like '*tuning*') {
    'cargo build --release --features tuning'
} else {
    'cargo build --release'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$source = Join-Path $repoRoot 'target/release/rsshogi-nnue-mini.exe'
$dist = Join-Path $repoRoot 'dist'
$destination = Join-Path $dist $name

if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "release binary not found: $source; run $build first"
}
if (-not (Test-Path -LiteralPath $dist -PathType Container)) {
    New-Item -ItemType Directory -Path $dist | Out-Null
}
Copy-Item -LiteralPath $source -Destination $destination -Force
Write-Output $destination
