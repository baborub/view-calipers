param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$packageRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $packageRoot
try {
    if (!$SkipBuild) {
        cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed' }
    }
    $metadata = cargo metadata --no-deps --locked --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
    $version = ($metadata.packages | Where-Object name -eq 'view-calipers').version
    $binary = Join-Path $packageRoot 'target/release/view-calipers.exe'
    if (!(Test-Path -LiteralPath $binary)) { throw 'Release executable is missing' }
    $dist = Join-Path $packageRoot 'dist'
    New-Item -ItemType Directory -Path $dist -Force | Out-Null
    $name = "view-calipers-v$version-windows-x64"
    $stage = Join-Path $dist "$name-$([Guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $stage | Out-Null
    try {
        Copy-Item -LiteralPath $binary -Destination (Join-Path $stage 'view-calipers.exe')
        foreach ($file in @('README.md','LICENSE','THIRD_PARTY.md','UnRAR-LICENSE.txt','CHANGELOG.md')) {
            Copy-Item -LiteralPath (Join-Path $packageRoot $file) -Destination $stage
        }
        Copy-Item -LiteralPath (Join-Path $packageRoot 'licenses') -Destination $stage -Recurse
        $zip = Join-Path $dist "$name.zip"
        Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -Force
        $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $name.zip" | Set-Content -LiteralPath (Join-Path $dist 'SHA256SUMS.txt') -Encoding ascii
        Write-Output "Release package: $zip"
        Write-Output "SHA256: $hash"
    } finally {
        $checkedStage = [IO.Path]::GetFullPath($stage)
        if (!$checkedStage.StartsWith([IO.Path]::GetFullPath($dist) + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe staging path' }
        Remove-Item -LiteralPath $checkedStage -Recurse -Force
    }
} finally { Pop-Location }
