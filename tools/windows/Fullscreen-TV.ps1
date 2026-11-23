param(
 [string]$Receiver=(Join-Path $PSScriptRoot 'airplay.exe'),
 [string]$GStreamer='C:\msys64\ucrt64\bin\gst-launch-1.0.exe',
 [string]$StateDirectory=(Join-Path $env:LOCALAPPDATA 'AirPlay\Windows-TV\mirror'),
 [string]$Name='Windows Screen Mirror'
)
$ErrorActionPreference='Stop'
$viewer=Join-Path (Split-Path $Receiver) 'airplay-video.exe'
foreach($file in @($Receiver,$GStreamer,$viewer)){if(!(Test-Path -LiteralPath $file -PathType Leaf)){throw "Missing executable: $file"}}
foreach($value in @($Name,$StateDirectory,$GStreamer)){if($value.Contains('"')){throw 'Quotes are not allowed in paths or name.'}}
[IO.Directory]::CreateDirectory($StateDirectory) | Out-Null
$stopFile=Join-Path $StateDirectory 'stop-fullscreen'
if(Test-Path -LiteralPath $stopFile){throw "Previous stop sentinel exists: $stopFile"}
$info=New-Object Diagnostics.ProcessStartInfo
$info.FileName=$Receiver
$info.Arguments='--name "'+$Name+'" --model AppleTV3,2 --source-version 220.68 --mirror --audio null --state "'+$StateDirectory+'" --video-player "'+$viewer+'" --video-player-framed'
$info.UseShellExecute=$false;$info.CreateNoWindow=$true
$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
$info.EnvironmentVariables['RUST_LOG']='info'
$info.EnvironmentVariables['PATH']=(Split-Path $GStreamer)+';'+$env:PATH
$process=New-Object Diagnostics.Process;$process.StartInfo=$info
$logs=@();$copies=@();$started=$false
try {
 if(!$process.Start()){throw 'Cannot start receiver.'};$started=$true
 $stamp=Get-Date -Format 'yyyyMMdd-HHmmss-fff'
 foreach($entry in @(@($process.StandardError.BaseStream,'receiver'),@($process.StandardOutput.BaseStream,'stdout'))){
  $file=New-Object IO.FileStream((Join-Path $StateDirectory ($stamp+'-'+$entry[1]+'.log')),[IO.FileMode]::Create,[IO.FileAccess]::Write,[IO.FileShare]::ReadWrite,1,[IO.FileOptions]::Asynchronous)
  $logs+=$file;$copies+=$entry[0].CopyToAsync($file)
 }
 @{receiver=$process.Id;supervisor=$PID;viewer='per-video-session'} | ConvertTo-Json | Set-Content (Join-Path $StateDirectory 'fullscreen-processes.json')
 Write-Host "Receiver ready: $Name; PID=$($process.Id). Viewer opens/closes with each video stream."
 while(!$process.HasExited -and !(Test-Path -LiteralPath $stopFile)){Start-Sleep -Milliseconds 250}
} finally {
 if($started){
  $children=Get-CimInstance Win32_Process -Filter "name='airplay-video.exe'" | Where-Object {$_.ParentProcessId -eq $process.Id}
  foreach($child in $children){Stop-Process -Id $child.ProcessId -ErrorAction SilentlyContinue}
  try{if(!$process.HasExited){$process.Kill();$process.WaitForExit(2000) | Out-Null}}catch{}
 }
 foreach($task in $copies){try{$task.Wait(2000) | Out-Null}catch{}}
 foreach($file in $logs){$file.Dispose()};$process.Dispose()
}

