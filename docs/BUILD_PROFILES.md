# Профили сборки

Стандартный профиль сохраняет зеркалирование и прежний API:

```sh
cargo build -p airplay --release
```

Устройства только со звуком собирают выбранный пакет без default features:

```sh
cargo build -p airplay --no-default-features --profile release-small
cargo test -p airplay --no-default-features
cargo tree -p airplay --no-default-features
```

В дереве этого пакета нет ap_mirror, ap_video, ap_hls, ap_capture и mediaserver. Команда для всего workspace продолжает собирать самостоятельные видео-компоненты; --no-default-features не удаляет их из workspace. Неиспользуемые прямые зависимости hls/mediaserver убраны из airplay.

Feature mirroring включает optional mirror/video и доступные только с ним поля Options/Context и re-export API. Audio-only CLI отвергает --mirror/--video-* и --display до старта служб, RTSP отвергает video stream110. Набор возможностей остаётся аудио. NTP realtime audio, AP2/PTP, MRP и sender не выключаются вместе с видео.

Feature ffmpeg включает codec/ffmpeg; weak video?/ffmpeg не включает видео сама. Проверка внешнего FFmpeg и всех целевых архитектур остаётся отдельной задачей. CI дополнена audio-only тестами и проверкой дерева зависимостей.

Проверены оба профиля на Pi/Linux в контейнере --network none:94 library +1 CLI в audio-only,94 application +79 sender +1 CLI в default; cargo check обоих без предупреждений. Установка на Станцию и проверка её звука не выполнялись.
