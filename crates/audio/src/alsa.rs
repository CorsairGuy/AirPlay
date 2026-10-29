//! ALSA через `dlopen("libasound.so.2")`: семь функций PCM API, без сборочных
//! зависимостей от alsa-lib.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::io;
use std::time::Duration;

use libloading::Library;
use tracing::{debug, warn};

use crate::{CHANNELS, Sink};

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

type Pcm = *mut c_void;

struct Api {
    open: unsafe extern "C" fn(*mut Pcm, *const c_char, c_int, c_int) -> c_int,
    set_params: unsafe extern "C" fn(Pcm, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int,
    writei: unsafe extern "C" fn(Pcm, *const c_void, c_ulong) -> c_long,
    delay: unsafe extern "C" fn(Pcm, *mut c_long) -> c_int,
    recover: unsafe extern "C" fn(Pcm, c_int, c_int) -> c_int,
    drop: unsafe extern "C" fn(Pcm) -> c_int,
    prepare: unsafe extern "C" fn(Pcm) -> c_int,
    close: unsafe extern "C" fn(Pcm) -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
    // Библиотека должна жить, пока живут указатели на её функции.
    _lib: Library,
}

impl Api {
    fn load() -> io::Result<Api> {
        let other = |e: libloading::Error| io::Error::other(format!("libasound: {e}"));
        // SAFETY: libasound — системная библиотека с C ABI; её конструкторы
        // не делают ничего, что нарушало бы инварианты процесса.
        let lib = unsafe { Library::new("libasound.so.2") }.map_err(other)?;
        // SAFETY: сигнатуры повторяют объявления из <alsa/pcm.h>.
        unsafe {
            Ok(Api {
                open: *lib.get(b"snd_pcm_open\0").map_err(other)?,
                set_params: *lib.get(b"snd_pcm_set_params\0").map_err(other)?,
                writei: *lib.get(b"snd_pcm_writei\0").map_err(other)?,
                delay: *lib.get(b"snd_pcm_delay\0").map_err(other)?,
                recover: *lib.get(b"snd_pcm_recover\0").map_err(other)?,
                drop: *lib.get(b"snd_pcm_drop\0").map_err(other)?,
                prepare: *lib.get(b"snd_pcm_prepare\0").map_err(other)?,
                close: *lib.get(b"snd_pcm_close\0").map_err(other)?,
                strerror: *lib.get(b"snd_strerror\0").map_err(other)?,
                _lib: lib,
            })
        }
    }

    fn error(&self, what: &str, code: c_int) -> io::Error {
        // SAFETY: snd_strerror возвращает статическую строку для любого кода.
        let msg = unsafe { CStr::from_ptr((self.strerror)(code)) };
        io::Error::other(format!("ALSA {what}: {}", msg.to_string_lossy()))
    }
}

pub struct Alsa {
    api: Api,
    pcm: Pcm,
    rate: u32,
}

// SAFETY: дескриптор PCM используется только из одного потока за раз (через
// &mut self), ALSA это допускает.
unsafe impl Send for Alsa {}

impl Alsa {
    pub fn open(device: &str, rate: u32, latency: Duration) -> io::Result<Alsa> {
        let api = Api::load()?;
        let name = CString::new(device).map_err(io::Error::other)?;
        let mut pcm: Pcm = std::ptr::null_mut();
        // SAFETY: указатели валидны на время вызова, pcm заполняется ALSA.
        let rc = unsafe { (api.open)(&mut pcm, name.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) };
        if rc < 0 {
            return Err(api.error("открытие", rc));
        }
        let latency_us = latency.as_micros().clamp(10_000, 2_000_000) as c_uint;
        // SAFETY: pcm открыт выше. soft_resample=1: plug-устройства сами
        // приведут частоту, если карта не умеет 44,1 кГц.
        let rc = unsafe {
            (api.set_params)(
                pcm,
                SND_PCM_FORMAT_S16_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                CHANNELS as c_uint,
                rate,
                1,
                latency_us,
            )
        };
        if rc < 0 {
            let e = api.error("настройка", rc);
            // SAFETY: закрываем то, что открыли.
            unsafe { (api.close)(pcm) };
            return Err(e);
        }
        debug!(device, rate, latency_us, "ALSA открыт");
        Ok(Alsa { api, pcm, rate })
    }
}

impl Sink for Alsa {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        let mut rest = samples;
        while !rest.is_empty() {
            let frames = (rest.len() / CHANNELS) as c_ulong;
            // SAFETY: буфер содержит frames * CHANNELS сэмплов i16.
            let n = unsafe { (self.api.writei)(self.pcm, rest.as_ptr().cast(), frames) };
            if n < 0 {
                // Обычно это недогрузка буфера (xrun): восстанавливаемся и пишем дальше.
                warn!("ALSA: {}", self.api.error("запись", n as c_int));
                // SAFETY: pcm открыт.
                let rc = unsafe { (self.api.recover)(self.pcm, n as c_int, 1) };
                if rc < 0 {
                    return Err(self.api.error("восстановление", rc));
                }
                continue;
            }
            rest = &rest[n as usize * CHANNELS..];
        }
        Ok(())
    }

    fn delay_frames(&mut self) -> u32 {
        let mut delay: c_long = 0;
        // SAFETY: pcm открыт, delay — валидный указатель.
        let rc = unsafe { (self.api.delay)(self.pcm, &mut delay) };
        if rc < 0 { 0 } else { delay.max(0) as u32 }
    }

    fn stop(&mut self) {
        // SAFETY: pcm открыт. После drop нужен prepare, чтобы снова писать.
        unsafe {
            (self.api.drop)(self.pcm);
            (self.api.prepare)(self.pcm);
        }
    }
}

impl Drop for Alsa {
    fn drop(&mut self) {
        // SAFETY: pcm открыт и больше не используется.
        unsafe { (self.api.close)(self.pcm) };
    }
}
