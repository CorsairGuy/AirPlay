# Mirroring TX: очистка после частичного SETUP

26 ноября 2026. Ветка `feat/mirror-partial-setup` от `origin/main` `b612eb4`.

Аппаратной проверки нет: Apple TV и iPhone недоступны.

## Что проверено

Loopback-приёмник `services/media/tests/mirror_tx.rs` уже покрывал: pair-verify, control-first `SETUP` звука (тип 96) с `sessionUUID`, `timingProtocol: NTP`, `timingPort`, `usingScreen`, `ct: 8`, `shk`; fallback на второй audio descriptor без `streamConnections` и с `controlPort` после отказа `415`; `SETUP` видео (тип 110) с `latencyMs`; `RECORD`; шифрование видео и звука; обнаружение обрыва видеоканала без ожидания кадра; `reconnect()` с новыми stream id и свежим `shk`; `stop()` с `TEARDOWN`.

Не было проверки отказного пути: что происходит, если видео-`SETUP` отклонён уже после удачного аудио-`SETUP`.

## Добавлено

`rejected_video_setup_tears_down_the_partial_session` — приёмник принимает pair-verify и аудио-`SETUP`, на видео-`SETUP` отвечает `500`, затем обязан получить `TEARDOWN`. Тест подтверждает, что:

- `MirrorSession::connect` возвращает ошибку с контекстом `video SETUP`;
- `TEARDOWN` приходит с тем же `Session`, что `sessionUUID`, объявленный в аудио-`SETUP` (то есть рвётся именно начатая сессия, а не выдуманная);
- `RECORD` после отказа не отправляется — тест падает, если он придёт.

Сама очистка существовала в коде (`TEARDOWN` в ветке ошибки `connect`), теперь она закреплена тестом, а не только чтением.

## Что осталось открытым

- `GET /info` перед `SETUP` отправитель не делает; сверка с doubletake `5e2f1cbe` не завершена.
- Жизненный цикл event channel: покрыт только ответ на зашифрованный запрос (`encrypted_event_requests_are_acknowledged`); переподключение event-канала и ошибки приёма не тестировались.
- `skipRecord` в харнессе всегда `true` для первого раунда; вариант «приёмник требует `RECORD` для обоих потоков» проверен только по коду.
- Геометрия, FairPlay, PTP и реальный приём картинки Apple TV — аппаратная проверка отсутствует.

## Проверка

```
cargo test --offline -p media --test mirror_tx   2 passed
cargo fmt -p media -- --check                    новых расхождений нет (прежние в raop.rs/rx.rs)
```
