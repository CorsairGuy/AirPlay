//! D03: один сценарий FLUSH по фактам shairport-sync 5.5.2
//! `player.c::do_flush` — «flush all packets up to, but not including, this
//! one»: граница остаётся, более ранние и опоздавшие пакеты не возвращаются,
//! 32-битная метка разворачивается, а новый поток продолжает очередь.

use std::sync::Arc;

use audio_stream::{Config, Playout};

fn playout() -> Playout {
    Playout::start(
        Config {
            device: "null".into(),
            ..Default::default()
        },
        Arc::new(clock::NoTime),
        None,
    )
}

#[test]
fn flush_keeps_the_boundary_rejects_late_packets_and_handles_wrap_and_restart() {
    let ordinary = playout();
    for ts in [1000u32, 2000, 3000] {
        ordinary.push(ts, 0x1600_0000, vec![1]);
    }

    // Граница 2000: пакет 2000 и более поздний остаются, 1000 выброшен.
    ordinary.flush_classic(2000);
    assert_eq!(ordinary.buffered_packets(), 2);

    // Опоздавший пакет из выброшенной части не возвращается.
    ordinary.push(1000, 0x1600_0000, vec![2]);
    assert_eq!(ordinary.buffered_packets(), 2);

    // Отдельный поток для rollover: метки до 2^32 несовместимы со шкалой
    // предыдущего потока, как и в реальном повторном SETUP.
    let wrapped = playout();
    for ts in [u32::MAX - 10, 5, 20] {
        wrapped.push(ts, 0x1600_0000, vec![3]);
    }
    wrapped.flush_classic(5);
    assert_eq!(
        wrapped.buffered_packets(),
        2,
        "до границы rollover выброшено"
    );

    wrapped.push(u32::MAX - 10, 0x1600_0000, vec![4]);
    assert_eq!(
        wrapped.buffered_packets(),
        2,
        "пакет до границы не вернулся"
    );

    wrapped.push(30, 0x1600_0000, vec![5]);
    assert_eq!(
        wrapped.buffered_packets(),
        3,
        "новый поток продолжает очередь"
    );
}
