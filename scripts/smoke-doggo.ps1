# Uses upstream's existing capture hooks; never changes the real taskbar or startup.
param([switch]$SkipBuild, [ValidateSet('reel', 'dock-typing', 'launch-dock', 'tray-access', 'dock-controls')][string]$Scenario = 'reel', [ValidateSet('bottom', 'center')][string]$Position = 'bottom')
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
$names = @('APPDATA', 'WISP_CAPTURE', 'WISP_CAPTURE_SCENARIO', 'WISP_CAPTURE_DURATION_MS', 'WISP_PERF_TRACE')
$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
try {
    if (-not $SkipBuild) {
        cargo +stable build --release --example launcher_demo --features perf-trace --locked
        if ($LASTEXITCODE -ne 0) { throw 'Instrumented build failed.' }
    }
    $testRoot = Join-Path $repo ('target\capture-' + [Guid]::NewGuid().ToString('N'))
    $demoData = Join-Path $testRoot 'wisp-demo'
    New-Item -ItemType Directory -Path $demoData -Force | Out-Null
    @('dock=true', 'hide_taskbar=false', "position=`"$Position`"", 'windows_key_search=false') | Set-Content -Encoding utf8 (Join-Path $demoData 'config.toml')
    $env:APPDATA = $testRoot
    $env:WISP_CAPTURE = '1'
    $env:WISP_CAPTURE_SCENARIO = $Scenario
    $env:WISP_CAPTURE_DURATION_MS = '6500'
    $env:WISP_PERF_TRACE = Join-Path $testRoot 'trace.csv'
    $capture = Start-Process -FilePath (Join-Path $repo 'target\release\examples\launcher_demo.exe') -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $testRoot 'stderr.log')
    if (-not $capture.WaitForExit(20000)) { throw 'Capture did not reach its automatic shutdown timer.' }
    if ($capture.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $env:WISP_PERF_TRACE)) { throw "Capture failed; see $testRoot" }
    $trace = Import-Csv -LiteralPath $env:WISP_PERF_TRACE
    if (-not ($trace | Where-Object metric -EQ 'ready')) { throw 'Capture never reached the ready checkpoint.' }
    $expectedFrame = if ($Scenario -eq 'dock-controls') { 'volume_popup_frame' } else { 'content_frame' }
    if (-not ($trace | Where-Object metric -EQ $expectedFrame)) { throw "Capture never presented its expected frame: $expectedFrame" }
    if ($Scenario -eq 'dock-typing' -and -not ($trace | Where-Object metric -EQ 'dock_typing_verified')) { throw 'Dock typing assertions did not complete.' }
    if ($Scenario -eq 'launch-dock' -and -not ($trace | Where-Object metric -EQ 'normal_launch_verified')) { throw 'Real second-process launch assertions did not complete.' }
    if ($Scenario -eq 'tray-access' -and -not ($trace | Where-Object metric -EQ 'tray_access_verified')) { throw 'Native tray access assertions did not complete.' }
    if ($Scenario -eq 'dock-controls' -and -not ($trace | Where-Object metric -EQ 'dock_controls_verified')) { throw 'Dock controls assertions did not complete.' }
    @('PASS: Optimized launcher initialized.', 'PASS: Scripted dock/launcher scenario presented content frames.', 'PASS: Automatic shutdown completed successfully.') | Set-Content -LiteralPath (Join-Path $testRoot 'result.txt')
    Get-Content -LiteralPath (Join-Path $testRoot 'result.txt')
    Write-Host "Capture evidence: $testRoot"
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    Pop-Location
}
