//! AAC-LC от FFmpeg (0,5 с, 440 Гц, стерео): кадры без ADTS, как их шлёт iPhone.

use ap_codec::{Decoder, Format};

/// Режет поток ADTS на сырые кадры AAC.
pub fn adts_frames(mut data: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    while data.len() > 7 && data[0] == 0xFF && data[1] & 0xF0 == 0xF0 {
        let len =
            ((data[3] as usize & 3) << 11) | ((data[4] as usize) << 3) | (data[5] as usize >> 5);
        let header = if data[1] & 1 == 0 { 9 } else { 7 };
        frames.push(&data[header..len]);
        data = &data[len..];
    }
    frames
}

#[test]
fn decodes_sine() {
    let frames = adts_frames(include_bytes!("data/sine440.adts"));
    assert!(frames.len() > 20);
    let mut d = Decoder::new(Format::from_ssrc(0x1600_0000).unwrap()).unwrap();
    let mut pcm = Vec::new();
    for f in &frames {
        assert_eq!(d.decode(f, &mut pcm).unwrap(), 1024);
    }
    // Середина: синус 440 Гц — около 100 периодов в секунде, проверим пересечения нуля.
    let left: Vec<i16> = pcm
        .iter()
        .step_by(2)
        .copied()
        .skip(4096)
        .take(44100 / 10)
        .collect();
    let crossings = left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
    assert!(
        (80..=96).contains(&crossings),
        "пересечений нуля: {crossings}"
    );
    let peak = left.iter().map(|s| s.unsigned_abs()).max().unwrap();
    // Генератор lavfi даёт амплитуду около 1/8 полной шкалы.
    assert!(peak > 2000, "сигнал слишком тихий: {peak}");
}
