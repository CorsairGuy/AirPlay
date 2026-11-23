# Windows TV receiver candidate

This package runs the universal core directly on Windows; it is independent of
Station deployment and does not complete the Station migration by itself.

Run `Start-TV.cmd`. It launches `airplay.exe` with the real LG model string
`OLED55C4RLA.DRUQLJP` observed in our reference capture, an independent identity,
a persistent state folder, the existing MRP card and default WASAPI audio output.
This is a diagnostic presentation profile, not LG firmware or an MFi identity.
No native LG feature mask, certificate, serial number or pairing key is copied.
The receiver advertises its existing implemented audio capabilities. Video and
mirroring are excluded from this build. A TV icon in iOS is a required manual
acceptance check and has not yet been established for this software profile.

## Use

- `Start-TV.cmd`: audio on the Windows default output and receiver card enabled.
- `Start-TV.cmd -Silent`: decode without audible output for card/discovery checks.
- `Start-TV.cmd -Name "Windows TV" -Port 7001`: change name/listener port.
- `Start-TV.cmd -NoCard`: isolate ordinary audio reception from MRP presentation.
- Ctrl+C stops the receiver. No service or startup entry is installed.

State and pairing keys: `%LOCALAPPDATA%\AirPlay\Windows-TV`.
Logs are under its `logs` subfolder and may contain pairing PIN and media metadata;
inspect them before sharing. Retain the state folder across restarts.

If Windows requests network permission, allow the receiver on the intended
private LAN. The launcher never modifies firewall rules or router settings.
Bonjour is provided by the existing Rust implementation; an Apple Bonjour
service is not a dependency. Other receivers may still conflict on a TCP port.

## Build

On Windows with Rust stable and MSVC C++ build tools:

```powershell
./tools/windows/Build-TV.ps1
```

The script performs one release build (`--no-default-features`), copies the exe,
launcher, license and documentation to a new directory, and writes SHA256SUMS.
It does not run tests, open listeners or start playback. The GitHub Actions
`Windows TV package` workflow publishes the same files as an artifact.

## Acceptance before claiming readiness

1. iPhone shows a television icon rather than an Apple TV/set-top-box icon.
   Merely changing the displayed model name does not prove this.
2. Quiet AirPlay reception, volume and Pause/Play work; restart preserves identity.
3. Second-iPhone card opens, updates track/artwork and controls the sender.
4. Pausing retains the AirPlay track until actual disconnection.

If the icon differs, retain a screenshot and the actual emitted `/info`/TXT.
Do not enable MFi, video or arbitrary feature bits merely to force an icon.
This package does not yet supply a desktop video window or a native GUI.
