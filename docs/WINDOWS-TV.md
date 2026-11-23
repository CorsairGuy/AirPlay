# Windows TV receiver candidate

This package runs the universal core directly on Windows; it is independent of
Station deployment and does not complete the Station migration by itself.

Run `Start-TV.cmd`. It launches `airplay.exe` with the real LG model string
`OLED55C4RLA.DRUQLJP` observed in our reference capture, an independent identity,
a persistent state folder, the existing MRP card and default WASAPI audio output.
This is a diagnostic presentation profile, not LG firmware or an MFi identity.
No native LG feature mask, certificate, serial number or pairing key is copied.
The receiver advertises its existing implemented audio capabilities. The default build includes the existing legacy mirroring transport; the
audio launcher does not enable it. A TV icon in iOS is a required manual
acceptance check and has not yet been established for this software profile.

## Use

- `Start-TV.cmd`: audio on the Windows default output and receiver card enabled.
- `Start-TV.cmd -Silent`: decode without audible output for card/discovery checks.
- `Start-TV.cmd -Name "Windows TV" -Port 7001`: change name/listener port.
- `Start-TV.cmd -NoCard`: isolate ordinary audio reception from MRP presentation.
- `Start-Mirror.cmd`: separate silent mirroring identity for icon/discovery checks;
  receives and counts video frames, but does not show an image.
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

The script performs one release build (including the default mirroring feature), copies the exe,
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

## Mirroring discovery experiment

The original audio-only LG-model trial showed a speaker icon. Changing
sourceVersion to 550.10 also showed a speaker. Model/sourceVersion alone
therefore did not achieve TV presentation in these user checks.

Start-Mirror.cmd activates the already implemented mirror profile, retaining
the LG model instead of substituting AppleTV3,2. Its keys live under the
separate `mirror` state subfolder, to distinguish it from the audio trial.
It currently counts/discards frames: no display window, modern Telik path
or combined AP2-audio/mirroring capability is claimed. This is a specific
discovery experiment, not a completed video receiver application.

## Fullscreen output on a host with GStreamer

`Fullscreen-TV.ps1` connects the existing Annex-B video output to GStreamer
through a binary pipe (no PowerShell text pipeline). It uses parsebin/decodebin
and Direct3D11 fullscreen output, retaining aspect ratio. GStreamer selects an
available decoder; hardware support depends on the host. No other AirPlay
transport or UxPlay library is loaded.

```powershell
./Fullscreen-TV.ps1 -GStreamer C:\msys64\ucrt64\bin\gst-launch-1.0.exe
```

GStreamer runtime/plugins are required separately; not included in this package.
The current launcher uses the confirmed AppleTV3,2 / 220.68 discovery profile,
a silent audio output, and the existing legacy mirror receiver. A live image
must still be confirmed on the actual sender. This does not add the Telik modern
video cipher path. Alt+Enter toggles fullscreen. The viewer is owned by each video session; disconnection closes it while
the receiver remains discoverable. creating `stop-fullscreen` in the state directory also stops both.
It is a low-latency display path, not a calibrated audio/video synchronization
claim. The bounded queue does not discard arbitrary compressed frames.

Popyachsa reference read: Recluse/Popyachsa-AirPlay at
5497138ed8b2fb5106df1532d325e30f63baf9ed, uxplay-patches README and full diff.
Provenance: declared GPL-3.0-or-later. Used as architecture/behaviour reference;
no external implementation code was copied. Its native HWND and orientation
patches are not yet ported into this display launcher.

