# Hardware audio rate estimator

<!-- current-status-20261112 -->
> Сверено12ноября2026. Текущий статус и ограничения — [STATUS](STATUS.md). Датированные замеры и этапы ниже сохранены как история, их слова «ещё не реализовано/не установлено» относятся к указанному этапу, а не автоматически к нынешнему main.


`ap_clock::AudioRateEstimator` measures frames per second using cumulative
**presented** frame counts and timestamps from one monotonic clock domain.
It is independent of the audio backend and does not change playback, packet
pacing, NTP anchors, local gain, or resampling.

The backend must extend a wrapping hardware counter to u64, reset the estimator
on device/format changes, and provide actual presentation timestamps rather than
the time a software queue was written. Do not mix process-relative `now_ns()`
with absolute kernel CLOCK_MONOTONIC timestamps.

Measurements use disjoint windows of at least one second. Five rates are held
in fixed memory; the median becomes usable after three valid windows. Rates
outside nominal ±1.5% are rejected. Counter regression, nonincreasing timestamp,
more than two seconds between samples, or more than 500ms without frame
advancement discard the estimate. An estimate older than two seconds is absent;
`rate_or_nominal` explicitly falls back to the configured sample rate.

This detects oscillator drift; it does not determine speaker acoustic latency.
It is not a PTP implementation and cannot by itself synchronize two speakers.
Integration still needs a verified hardware timestamp source, a shared media
epoch, and an explicit decision about rate correction/ASRC. Changing only the
network send interval would make packet timing and media timestamps disagree.

## Research provenance

The idea of measuring actual audio clock frequency and filtering recent rate
measurements was studied in [Telik AudioMasterClock.kt, pinned revision
eec6e341](https://github.com/andrewkomkov/telik/blob/eec6e34151c2447c58df4e187411ffe72031a94d/app/src/main/kotlin/io/github/henry753951/airplayremote/audiooutput/protocol/AudioMasterClock.kt).
That protocol subtree declares Apache-2.0 and credits AirBridge (revision
12d4fa781b86aaac6135e9279044d8a0f81fce21), which credits airplay2-sender-cpp.
No Kotlin implementation was copied; this Rust estimator uses its own fixed
storage, disjoint windows, lifecycle rules and tests under this repository's
license. Android silent AudioTrack creation and scheduling are not ported.

The additional card regression in `station_mrp/control.rs` covers frozen paused
elapsed time, metadata preservation and a subsequent sender Play overriding a
controller pause. Scenario selection was informed by
[AppleTVControlLibrary player-state tests](https://github.com/oznetmaster/AppleTVControlLibrary/blob/4d416ff8c754e9eddcc423bbda0cffe99778e0d1/tests/AppleTv.Mrp.Tests/PlayerState/MrpPlayerStateManagerTests.cs),
whose header also attributes pyatv 0.18.0. No C# source was copied. Their
multi-player client state model is not equivalent to this single-source server;
those tests have not been claimed as implemented here.
