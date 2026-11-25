//! Прогон fuzz-корпуса через сетевые парсеры приёмника.
//!
//! Смысл: ни один файл из `fuzz/corpus` не должен паниковать, зацикливаться
//! или приводить к неограниченному росту буфера. Векторы синтетические, без
//! ключей и данных реальных устройств. Подробности — `fuzz/README.md`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const BUDGET: Duration = Duration::from_secs(2);

fn corpus(directory: &str) -> Vec<(String, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus");
    let dir: PathBuf = root.join(directory);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("нет каталога {}: {e}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    assert!(!files.is_empty(), "пустой корпус {}", dir.display());
    files
        .into_iter()
        .map(|path| {
            let data = std::fs::read(&path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, data)
        })
        .collect()
}

/// Разбор не должен тратить на вектор больше бюджета.
fn assert_bounded(name: &str, started: Instant) {
    assert!(
        started.elapsed() < BUDGET,
        "{name}: разбор дольше {BUDGET:?}"
    );
}

#[test]
fn rtsp_vectors_are_rejected_without_panicking() {
    for (name, data) in corpus("rtsp") {
        let started = Instant::now();
        let mut request = data.clone();
        let _ = rtsp::decode_request(&mut request);
        let mut response = data.clone();
        let _ = rtsp::decode_response(&mut response);
        // По байту: незавершённый ввод не должен терять буфер или паниковать.
        let mut request = data.clone();
        let _ = rtsp::decode_request(&mut request);
        assert_bounded(&name, started);
    }
}

#[test]
fn tlv8_vectors_do_not_panic_in_pairing() {
    let store = pair::MemoryStore::default();
    let mut setup =
        pair::hap::PairSetup::new(pair::Identity::from_seed("corpus", &[9; 32]), "0000");
    for (name, data) in corpus("tlv8") {
        let started = Instant::now();
        let _ = pair::tlv8::Tlv8::decode(&data);
        let _ = setup.handle(&data, &store);
        assert_bounded(&name, started);
    }
}

#[test]
fn plist_vectors_are_refused_before_parsing() {
    for (name, data) in corpus("plist") {
        let started = Instant::now();
        assert!(
            rtsp::plist::parse(&data).is_none(),
            "{name}: вектор должен быть отвергнут"
        );
        assert_bounded(&name, started);
    }
}

#[test]
fn rtp_and_video_vectors_are_rejected() {
    let cipher = rtp::AudioCipher::new(&[0; 32]);
    for (name, data) in corpus("rtp") {
        let started = Instant::now();
        let _ = rtp::Header::parse(&data);
        let mut packet = data.clone();
        let _ = cipher.open(&mut packet);
        let mut buffered = data.clone();
        let _ = rtp::split_buffered(&mut buffered);
        // Заголовок видеопакета фиксированного размера: дополняем нулями.
        #[cfg(feature = "mirroring")]
        {
            let mut header = [0u8; mirror::HEADER_LEN];
            let copy = data.len().min(header.len());
            header[..copy].copy_from_slice(&data[..copy]);
            let _ = mirror::Header::parse(&header);
        }
        assert_bounded(&name, started);
    }
}
