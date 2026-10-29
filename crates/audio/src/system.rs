//! Вывод через cpal (WASAPI на Windows, CoreAudio на macOS).
//!
//! Поток cpal на некоторых системах нельзя передавать между потоками, поэтому
//! он живёт в своём потоке, а данные идут через очередь. Частоту карты
//! (обычно 48 кГц) догоняем линейной интерполяцией прямо в обратном вызове.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tracing::{debug, warn};

use crate::{CHANNELS, Sink};

struct Shared {
    queue: Mutex<VecDeque<i16>>,
    space: Condvar,
    capacity: usize,
    /// Кадры внутри буфера карты (оценка по размеру последнего обратного вызова).
    device_frames: AtomicU32,
    closed: AtomicBool,
}

pub struct System {
    shared: Arc<Shared>,
    rate: u32,
}

impl System {
    pub fn open(device: &str, rate: u32, latency: Duration) -> io::Result<System> {
        let capacity = (latency.as_millis().max(20) as usize * rate as usize / 1000) * CHANNELS;
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            space: Condvar::new(),
            capacity,
            device_frames: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        });
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread_shared = shared.clone();
        let name = device.to_string();
        std::thread::Builder::new()
            .name("audio-out".into())
            .spawn(move || run(&name, rate, thread_shared, ready_tx))?;
        ready_rx
            .recv()
            .map_err(|_| io::Error::other("поток вывода звука завершился"))??;
        Ok(System { shared, rate })
    }
}

fn run(name: &str, rate: u32, shared: Arc<Shared>, ready: std::sync::mpsc::Sender<io::Result<()>>) {
    let stream = match build(name, rate, shared.clone()) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let lock = Mutex::new(());
    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !shared.closed.load(Ordering::Relaxed) {
        guard = shared
            .space
            .wait_timeout(guard, Duration::from_millis(200))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    drop(stream);
}

fn build(name: &str, rate: u32, shared: Arc<Shared>) -> io::Result<cpal::Stream> {
    let err = |e: &dyn std::fmt::Display| io::Error::other(format!("звук: {e}"));
    let host = cpal::default_host();
    let device = if name == "default" {
        host.default_output_device()
    } else {
        host.output_devices()
            .map_err(|e| err(&e))?
            .find(|d| d.description().is_ok_and(|n| n.name() == name))
    }
    .ok_or_else(|| io::Error::other(format!("звук: нет устройства {name}")))?;
    let config: cpal::StreamConfig = device.default_output_config().map_err(|e| err(&e))?.into();
    let out_channels = config.channels as usize;
    let step = rate as f64 / config.sample_rate as f64;
    debug!(?config, "cpal: открыт вывод");

    let mut pos = 0f64;
    let mut prev = [0f32; CHANNELS];
    let mut next = [0f32; CHANNELS];
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out, _| {
                let frames = out.len() / out_channels;
                shared.device_frames.store(frames as u32, Ordering::Relaxed);
                let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
                for frame in out.chunks_exact_mut(out_channels) {
                    while pos >= 1.0 {
                        pos -= 1.0;
                        prev = next;
                        if q.len() >= CHANNELS {
                            for s in &mut next {
                                *s = q.pop_front().unwrap_or(0) as f32 / 32768.0;
                            }
                        } else {
                            next = [0.0; CHANNELS];
                        }
                    }
                    let t = pos as f32;
                    for (c, s) in frame.iter_mut().enumerate() {
                        *s = if c < CHANNELS {
                            prev[c] + (next[c] - prev[c]) * t
                        } else {
                            0.0
                        };
                    }
                    pos += step;
                }
                drop(q);
                shared.space.notify_all();
            },
            |e| warn!("cpal: {e}"),
            None,
        )
        .map_err(|e| err(&e))?;
    stream.play().map_err(|e| err(&e))?;
    Ok(stream)
}

impl Sink for System {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        let mut rest = samples;
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        while !rest.is_empty() {
            let free = self.shared.capacity.saturating_sub(q.len());
            if free < CHANNELS {
                q = self
                    .shared
                    .space
                    .wait_timeout(q, Duration::from_millis(100))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
                continue;
            }
            let n = free.min(rest.len()) / CHANNELS * CHANNELS;
            q.extend(&rest[..n]);
            rest = &rest[n..];
        }
        Ok(())
    }

    fn delay_frames(&mut self) -> u32 {
        let queued = self.shared.queue.lock().map_or(0, |q| q.len() / CHANNELS) as u32;
        queued + self.shared.device_frames.load(Ordering::Relaxed)
    }

    fn stop(&mut self) {
        if let Ok(mut q) = self.shared.queue.lock() {
            q.clear();
        }
    }
}

impl Drop for System {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        self.shared.space.notify_all();
    }
}
