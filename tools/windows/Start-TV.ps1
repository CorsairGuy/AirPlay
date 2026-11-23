param(
    [string]$Name = "$env:COMPUTERNAME TV",
    [string]$Model = 'OLED55C4RLA.DRUQLJP',
    [ValidateRange(1,65535)][int]$Port = 7000,
    [switch]$Silent,
    [switch]$NoCard
)
$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot 'airplay.exe'
if (!(Test-Path -LiteralPath $exe)) { throw "airplay.exe is missing beside this script." }
if (!$env:LOCALAPPDATA) { throw 'LOCALAPPDATA is unavailable.' }
$state = Join-Path $env:LOCALAPPDATA 'AirPlay\Windows-TV'
$logs = Join-Path $state 'logs'
[IO.Directory]::CreateDirectory($logs) | Out-Null
$log = Join-Path $logs ((Get-Date -Format 'yyyyMMdd-HHmmss-fff') + '.log')
$receiverArgs = @('--name',$Name,'--model',$Model,'--port',"$Port",'--state',$state)
if (!$NoCard) { $receiverArgs += '--mrp' }
if ($Silent) { $receiverArgs += @('--audio','null') }
Write-Host "Receiver: $Name; model: $Model"
Write-Host "State: $state"
Write-Host "Log: $log"
Write-Host 'Stop with Ctrl+C. This script does not change firewall or router settings.'
Write-Host 'The TV icon is an iOS interoperability check; no LG MFi capabilities are copied.'
if (!$env:RUST_LOG) { $env:RUST_LOG = 'info' }
$ErrorActionPreference = 'Continue'
& $exe @receiverArgs 2>&1 | Tee-Object -FilePath $log
exit $LASTEXITCODE
