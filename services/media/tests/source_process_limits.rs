//! Внешний процесс источника: валидация, завершение и отсутствие потомков (C55).
//!
//! Используется только системная программа, запускающая короткий сон; пути и
//! настройки пользователя не читаются.

use std::time::Duration;

use media::tx::source_process::{Capture, Options};

#[test]
fn invalid_commands_are_rejected_before_spawn() {
    let bad = [
        Options {
            program: "relative/tool".into(),
            args: vec![],
        },
        Options {
            program: std::path::PathBuf::from(if cfg!(windows) { "C:\\tool.exe" } else { "/tool" }),
            args: (0..65).map(|i| i.to_string()).collect(),
        },
        Options {
            program: std::path::PathBuf::from(if cfg!(windows) { "C:\\tool.exe" } else { "/tool" }),
            args: vec!["a".repeat(4097)],
        },
        Options {
            program: std::path::PathBuf::from(if cfg!(windows) { "C:\\tool.exe" } else { "/tool" }),
            args: vec!["zero\0byte".into()],
        },
    ];
    for options in bad {
        assert!(options.validate().is_err(), "{options:?}");
    }
    let good = Options {
        program: std::path::PathBuf::from(if cfg!(windows) { "C:\\tool.exe" } else { "/tool" }),
        args: vec!["-f".into(), "raw".into()],
    };
    assert!(good.validate().is_ok());
}

#[cfg(windows)]
fn sleeper(seconds: u32) -> Options {
    Options {
        program: "C:\\Windows\\System32\\PING.EXE".into(),
        args: vec!["-n".into(), seconds.to_string(), "127.0.0.1".into()],
    }
}

#[cfg(not(windows))]
fn sleeper(seconds: u32) -> Options {
    Options {
        program: "/bin/sleep".into(),
        args: vec![seconds.to_string()],
    }
}

#[cfg(windows)]
fn failing() -> Options {
    Options {
        program: "C:\\Windows\\System32\\cmd.exe".into(),
        args: vec!["/C".into(), "exit 3".into()],
    }
}

#[cfg(not(windows))]
fn failing() -> Options {
    Options {
        program: "/bin/false".into(),
        args: vec![],
    }
}

#[tokio::test]
async fn stop_reaps_the_child_within_the_deadline() {
    let capture = Capture::spawn(sleeper(60)).unwrap();
    let pid = capture.id().unwrap();
    assert!(pid > 0);
    tokio::time::timeout(Duration::from_secs(3), capture.stop())
        .await
        .expect("stop обязан уложиться в срок")
        .unwrap();
}

#[tokio::test]
async fn nonzero_exit_is_a_source_error_not_silence() {
    let mut capture = Capture::spawn(failing()).unwrap();
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    capture.read_to_end(&mut bytes).await.unwrap();
    assert!(bytes.is_empty());
    assert!(capture.finish().await.is_err(), "ненулевой код выхода — ошибка");
}

#[tokio::test]
async fn selected_capture_does_not_restart_a_running_source() {
    use media::tx::source_process::SelectedCapture;
    let mut selected = SelectedCapture::default();
    assert!(
        selected
            .select(sleeper(60))
            .await
            .expect("первый выбор"),
        "первый выбор запускает процесс"
    );
    assert!(
        !selected
            .select(sleeper(60))
            .await
            .expect("повторный выбор"),
        "тот же живой источник не перезапускается"
    );
    // Ошибочный вариант отклоняется до остановки рабочего источника.
    assert!(
        selected
            .select(Options {
                program: "relative".into(),
                args: vec![],
            })
            .await
            .is_err()
    );
    assert!(selected.reader().is_ok(), "рабочий источник остался выбранным");
    selected.stop().await.unwrap();
    assert!(selected.reader().is_err());
}
