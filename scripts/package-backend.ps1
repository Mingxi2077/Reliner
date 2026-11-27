#requires -Version 5.1
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu')]
    [string]$Target = 'x86_64-pc-windows-msvc',
    [string]$Toolchain = '1.96.0',
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..\dist')
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).ProviderPath
$buildRoot = Join-Path $projectRoot 'target'
$outputRoot = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$packageVersion = (Select-String -LiteralPath (Join-Path $projectRoot 'Cargo.toml') -Pattern '^version = "([^"]+)"$').Matches[0].Groups[1].Value
$archivePath = Join-Path $outputRoot "reliner-backend-$packageVersion-$Target.zip"
if (Test-Path -LiteralPath $archivePath) { throw "交付包已存在：$archivePath" }

function Invoke-Cargo {
    & cargo "+$Toolchain" @args
    if ($LASTEXITCODE -ne 0) { throw "构建检查未通过，退出码 $LASTEXITCODE。" }
}

$previousFlags = $env:RUSTFLAGS
$previousEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$stageRoot = $null
Push-Location -LiteralPath $projectRoot
try {
    if ($Target -eq 'x86_64-pc-windows-msvc') {
        if ($null -ne $previousEncodedFlags) {
            $separator = [char]0x1f
            $additional = "-C${separator}target-feature=+crt-static"
            $env:CARGO_ENCODED_RUSTFLAGS = if ($previousEncodedFlags.Length) { "$previousEncodedFlags$separator$additional" } else { $additional }
        } else {
            $env:RUSTFLAGS = "$previousFlags -C target-feature=+crt-static".Trim()
        }
    }
    Invoke-Cargo fmt --all '--' --check
    Invoke-Cargo clippy --locked --all-targets --target $Target --target-dir $buildRoot '--' -D warnings
    Invoke-Cargo test --locked --release --all-targets --target $Target --target-dir $buildRoot
    Invoke-Cargo build --locked --release --target $Target --target-dir $buildRoot --bin reliner

    $stageRoot = Join-Path ([IO.Path]::GetTempPath()) "reliner-package-$([guid]::NewGuid().ToString('N'))"
    [void][IO.Directory]::CreateDirectory($stageRoot)
    [void][IO.Directory]::CreateDirectory((Join-Path $stageRoot 'examples'))
    Copy-Item -LiteralPath (Join-Path $projectRoot "target\$Target\release\reliner.exe") -Destination $stageRoot
    Get-ChildItem -LiteralPath (Join-Path $projectRoot 'examples') -Filter '*.json' -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $stageRoot 'examples')
    }
    Copy-Item -LiteralPath (Join-Path $projectRoot 'examples\import-containerlab.yml') -Destination (Join-Path $stageRoot 'examples')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'examples\import-gns3.gns3') -Destination (Join-Path $stageRoot 'examples')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'examples\ios-trunk.cfg') -Destination (Join-Path $stageRoot 'examples')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'examples\ios-switch.cfg') -Destination (Join-Path $stageRoot 'examples')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'examples\lldp-peer.pcap') -Destination (Join-Path $stageRoot 'examples')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'examples\capture-frr-route.py') -Destination (Join-Path $stageRoot 'examples')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs\backend-release.md') -Destination (Join-Path $stageRoot 'README.md')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination $stageRoot
    [void][IO.Directory]::CreateDirectory($outputRoot)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($stageRoot, $archivePath)
    Write-Output "已生成 $archivePath"
}
finally {
    $env:RUSTFLAGS = $previousFlags
    $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags
    Pop-Location
    if ($stageRoot -and (Test-Path -LiteralPath $stageRoot)) {
        $resolvedStage = (Resolve-Path -LiteralPath $stageRoot).ProviderPath
        $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
        if (-not $resolvedStage.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or
            [IO.Path]::GetFileName($resolvedStage) -notlike 'reliner-package-*') {
            throw "临时打包目录不符合预期：$resolvedStage"
        }
        $examplesStage = Join-Path $resolvedStage 'examples'
        if (Test-Path -LiteralPath $examplesStage) {
            Get-ChildItem -LiteralPath $examplesStage -File | ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force }
            [IO.Directory]::Delete($examplesStage)
        }
        Get-ChildItem -LiteralPath $resolvedStage -File | ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force }
        [IO.Directory]::Delete($resolvedStage)
    }
}
