# Профили сборки

`bin/airplay` удалён по решению пользователя. Ядро предоставляет библиотечный пакет `runtime`; исполняемый файл создаёт использующее его приложение.

```sh
cargo build -p runtime --release
cargo test -p runtime --no-default-features
cargo test -p runtime --features mirroring
cargo test --workspace
```

По умолчанию runtime не включает зеркалирование; `mirroring` включает приём зеркальных кадров. `hls` требует FFmpeg и системные зависимости. Видео-декодирование с FFmpeg выбирает потребитель через `video/ffmpeg`; AAC через `codec/ffmpeg`. Наличие поддержки сборки не подтверждает работу на устройстве.

На Windows для MSYS2 UCRT64 см. [AGENT-TASKS](AGENT-TASKS.md). Ранее описанные сборки CLI, `airplay.exe` и Windows-TV-скрипты относятся к удалённой программе и не являются текущим способом запуска.
