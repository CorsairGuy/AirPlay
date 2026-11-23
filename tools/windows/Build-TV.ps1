param([string]$Destination)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
if (!$Destination) { $Destination = Join-Path $root 'target\windows-tv' }
$Destination = [IO.Path]::GetFullPath($Destination)
if (Test-Path -LiteralPath $Destination) { throw "Destination already exists: $Destination. Choose a new folder." }
Push-Location $root
try {
    & cargo build --locked --release -p airplay
    if ($LASTEXITCODE -ne 0) { throw 'Windows receiver build failed.' }
    $exe = Join-Path $root 'target\release\airplay.exe'
    if (!(Test-Path -LiteralPath $exe)) { throw 'Build did not produce a Windows airplay.exe.' }
    [IO.Directory]::CreateDirectory($Destination) | Out-Null
    Copy-Item -LiteralPath $exe -Destination $Destination
    foreach ($file in @('Start-TV.ps1','Start-TV.cmd','Start-Mirror.cmd','Fullscreen-TV.ps1')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $Destination
    }
    Copy-Item -LiteralPath (Join-Path $root 'docs\WINDOWS-TV.md') -Destination (Join-Path $Destination 'README.md')
    $license = Join-Path $root 'LICENSE'
    if (Test-Path -LiteralPath $license) { Copy-Item -LiteralPath $license -Destination $Destination }
    $hash = Get-FileHash -LiteralPath (Join-Path $Destination 'airplay.exe') -Algorithm SHA256
    ($hash.Hash.ToLowerInvariant() + '  airplay.exe') | Set-Content -Encoding ascii (Join-Path $Destination 'SHA256SUMS')
    if ($env:GITHUB_SHA) { $env:GITHUB_SHA | Set-Content -Encoding ascii (Join-Path $Destination 'COMMIT') }
    Write-Host "Windows package: $Destination"
} finally { Pop-Location }

