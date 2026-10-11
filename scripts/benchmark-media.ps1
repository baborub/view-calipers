param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\view-calipers.exe'),
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..\media-benchmark')
)

$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path -LiteralPath $Exe).Path
$runDirectory = Join-Path ([IO.Path]::GetFullPath($OutputDirectory)) ('run-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
$profileDirectory = Join-Path $runDirectory 'profile'
New-Item -ItemType Directory -Path $profileDirectory | Out-Null
$report = Join-Path $runDirectory 'gpu.json'
$stderr = Join-Path $runDirectory 'stderr.txt'

# Keep benchmark settings and diagnostic switches out of the user's normal profile.
$variables = @('LOCALAPPDATA', 'VIEW_CALIPERS_GPU_DIAGNOSTICS', 'VIEW_CALIPERS_MEDIA_LEGACY',
    'VIEW_CALIPERS_BENCH_FOLDER', 'VIEW_CALIPERS_BENCH_OUTPUT', 'ARCHVIEW_UI_CHECK', 'ARCHVIEW_UI_CAPTURE')
$saved = @{}
foreach ($name in $variables) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$child = $null
try {
    foreach ($name in $variables) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    $env:LOCALAPPDATA = $profileDirectory
    $env:VIEW_CALIPERS_GPU_DIAGNOSTICS = $report
    $child = Start-Process -FilePath $binary -WindowStyle Hidden -PassThru -RedirectStandardError $stderr
    if (-not $child.WaitForExit(60000)) {
        $child.Kill()
        $child.WaitForExit()
        throw "GPU benchmark timed out. Log: $stderr"
    }
    $child.Refresh()
    if ($child.ExitCode -ne 0) { throw "GPU benchmark exited with $($child.ExitCode). Log: $stderr" }
    if (-not (Test-Path -LiteralPath $report)) { throw "No diagnostic report produced. Log: $stderr" }
    $result = Get-Content -LiteralPath $report -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($result.error) { throw "GPU diagnostic failed: $($result.error). Log: $stderr" }
    if ($result.checks.Count -lt 6 -or $result.benchmarks.Count -lt 5) { throw 'Incomplete GPU checks' }
    Write-Output "GPU: $($result.gpu)"
    Write-Output "Checks: $($result.checks.Count) passed"
    Write-Output "Raw timings: $report"
    Write-Output $result.note
} finally {
    foreach ($name in $variables) {
        if ($null -eq $saved[$name]) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    }
    if ($null -ne $child) { $child.Dispose() }
}
