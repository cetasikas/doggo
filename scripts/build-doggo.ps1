param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
try {
    if (-not $SkipBuild) {
        cargo +stable build --release --workspace --locked
        if ($LASTEXITCODE -ne 0) { throw 'Doggo build failed.' }
    }
    $destination = Join-Path $repo 'dist\Doggo'
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    foreach ($binary in 'doggo.exe', 'doggo-settings.exe') {
        Copy-Item -LiteralPath (Join-Path $repo "target\release\$binary") -Destination $destination -Force
    }
    foreach ($document in 'LICENSE', 'README.md', 'CHANGELOG.md', 'DOGGO.md', 'VALIDATION.md', 'UPSTREAM.md') {
        Copy-Item -LiteralPath (Join-Path $repo $document) -Destination $destination -Force
    }
    Get-FileHash (Join-Path $destination '*.exe') | Format-Table -AutoSize
    Write-Host "Portable build: $destination"
    Write-Host 'No startup entry was created. Read DOGGO.md before running.'
} finally { Pop-Location }
