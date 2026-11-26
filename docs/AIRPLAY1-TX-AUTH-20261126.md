# AirPlay 1 TX: Apple-Response и `et` (P1, часть)

26 ноября 2026. Ветка `feat/airplay1-auth` от `origin/main` `b612eb4`.

Аппаратной проверки нет: AirPort Express, Apple TV и iTunes/Mac недоступны.

## Что проверялось

Вопрос из очереди: «действительно ли и в каких случаях `Apple-Response` должен проверяться; не придумывать проверку по названию заголовка». Референс — OwnTone `612d6b3cf34ae38bb71b6edfcd69fd7afe6ab25c`, `src/outputs/raop.c` (закреплённая в плане ревизия).

## Findings (по исходнику референса)

1. **`Apple-Response` отправитель не проверяет и не получает.** В `raop.c` этой ревизии нет ни одного вхождения `Apple-Response`. `Apple-Challenge` кладётся в `ANNOUNCE` (строка 1215 и далее), а ответ формирует приёмник. То есть у TX-стороны нет сценария, в котором заголовок надо проверять: наша прежняя формулировка «Apple-Response verification is not implemented here» намекала на пропущенную функцию, которой в протоколе для отправителя не существует.
2. **Challenge отправляется только для шифрованной сессии** (`if (rs->encrypt)` перед `evrtsp_add_header(..., "Apple-Challenge", ...)`), 16 случайных байт, base64 без padding. Наш код это уже делал: `STANDARD_NO_PAD.encode(rand::random::<[u8; 16]>())` при наличии `audio_key`.
3. **`et` в `_raop._tcp` не означает шифрование payload.** Референс разбирает только токен `4` — «support (!= requirement) for auth-setup» (строка 4448), а режим шифрования выбирает по модели устройства (строки 2223–2248), не по списку `et`. Наш `Receiver.encryption` вычислялся как «в `et` есть 1 или 4», то есть был догадкой по имени поля.

## Изменения — `services/media/src/tx/airplay1.rs`

- Документация модуля исправлена: challenge только для шифрованной сессии, `Apple-Response` формирует приёмник, ссылка на ревизию и строки референса, запрет придумывать проверку.
- `Receiver::encryption` заменён на `Receiver::auth_setup` — только токен `4` (auth-setup support), с той же ссылкой. `encryption_types` остаётся как объявленный список без толкования значений.
- Формирование challenge вынесено в `announce_challenge()`; поведение не изменилось.
- `SessionConfig::validate` по-прежнему отказывает в открытом аудио, если объявленный `et` не содержит `0`: это чтение объявленного списка, а не вывод о шифровании.
- Тест `announce_challenge_is_sixteen_unpadded_base64_bytes` фиксирует 22 символа без `=`, декодируется в 16 байт; тест объявления теперь проверяет `auth_setup` для `et=0,1` (false) и `et=0,4` (true).

## Проверка

```
cargo test --offline -p media --lib   127 passed
cargo check --offline -p airplay --tests   ok
cargo clippy --offline -p media --lib -- -D warnings   ok
cargo fmt -p media -- --check   замечания только в raop.rs/rx.rs (не мои файлы)
```

## Ограничения

- Само шифрование и RSA-OAEP упаковка ключа проверены только локальными тестами; независимых wire-векторов RSA/SDP в этой ветке нет — это остаток задачи.
- Значения `et`, кроме `4`, не толкуются: ни `1`, ни `3`, ни `5` не объявляются как подтверждённые режимы.
- Приём на реальном AirPort Express/Apple TV не проверялся; аппаратная проверка остаётся открытой.
