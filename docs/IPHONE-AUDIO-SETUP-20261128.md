# iPhone audio SETUP: сверка с референсами (D01, R01, R03, R04)

Проверка фактического `main` от 28 ноября 2026. Причина вчерашнего отказа
iPhone не установлена: ниже только подтверждённые референсами факты, наше
поведение и явные ограничения. Это не заявление о поддержке устройства или
исправлении звука.

## Источники

- **S** — shairport-sync 5.5.2, `rtsp.c`, функция `handle_setup_2`
  (https://github.com/mikebrady/shairport-sync/blob/5.5.2/rtsp.c).
  Номера строк — в этом файле ревизии.
- **U** — UxPlay 534e2cc0, `lib/raop_handlers.h`, обработчик SETUP
  (https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/raop_handlers.h).
- **P** — pyatv b277a4c8, HAP pairing (`pyatv/protocols/airplay/auth/hap.py`)
  — для контекста сопряжения; pyatv не описывает iPhone SETUP.
- Лицензии: S — GPL-3.0, U — GPL-3.0, P — MIT. Код не копировался.

## Два SETUP

S (строки 2410–2440) и U (строки 896–975) различают:

1. **Начальный SETUP без `streams`** — часы и канал событий.
   `timingProtocol` = `PTP` | `NTP` | `None`; для NTP отправитель присылает
   `timingPort`. S для PTP отвечает `eventPort` и фиктивным `timingPort: 0`;
   для remote-control-only — только `eventPort` (строки 2652–2655, 2718–2722).
   S вообще не обслуживает NTP-потоки (строка 2686: "can not handle NTP
   streams"); U принимает NTP и возвращает свой `timingPort` (строки 968–975).
2. **SETUP с `streams`** — по одному дескриптору на поток.

## Обязательные поля дескрипторов

| Тип | Поля запроса | Ответ | Референс |
|---|---|---|---|
| 96 realtime audio | `type`, `shk` (32 байта), `ct`, `sr`, `spf`, `ss`, `ch`, `controlPort` | `type`, `dataPort`, `controlPort` | S 2766–2920; U 1050–1090 |
| 103 buffered audio | `type`, `shk` (32 байта), `audioFormat`, `ct`, `sr`, `spf` | `type`, `dataPort`, `audioBufferSize` | S 2921–2970 |
| 110 video | `type`, `streamConnectionID`; ключ из `ekey` или HAP shared secret | `type`, `dataPort` | S 2870; U 1000–1048 |
| 130 remote control | `type`, `seed` | `type`, `streamID: 1`, `dataPort` при `seed` | S 2971–3010 |

Ключевые факты:

- S требует `shk` ровно 32 байта: при другой длине ключ отбрасывается
  (строки 2774–2784), но SETUP не отвергается. U `shk` не читает вовсе.
- S читает только `streams[0]`; наш приёмник обрабатывает все элементы.
- S для потоков 96/103 отвечает `dataPort` (+ `controlPort` у 96);
  `latencyMin` в ответе S нет — это наше поле, не референс.

## Наше поведение после проверки

- `services/media/src/rx.rs`: весь список `streams` проверяется до изменения
  состояния. Тип 96/103 без `shk` ровно 32 байта — отказ 500 целиком;
  частично занятый плеер и открытые сокеты освобождаются, повторный SETUP
  проходит (`failed_multi_stream_setup_leaves_no_active_stream_and_a_retry_succeeds`).
- Тип 96 без `shk` при включённом mirror — только при наличии `ekey` и `eiv`
  из начального SETUP; иначе явный отказ (не подставляем чужой ключ).
- NTP: отсутствующий, нулевой или >65535 `timingPort` не запускает обмен,
  ответ 200 без `timingPort`; корректный порт запускает обмен, TEARDOWN
  закрывает сокет (`ntp_setup_*` тесты).
- Фикстуры: `tests/interop/fixtures/airplay2-audio-setup.plist` (валидный
  тип 96) и `airplay2-audio-setup-short-shk.plist` (16-байтовый ключ).
  Это синтез по S/U, не захват iPhone; тест —
  `services/media/tests/audio_setup_fixture.rs`.

## Ограничения

- Захвата реального iPhone в репозитории нет, поэтому точная причина отказа
  `shk` вчера не доказана. Возможные варианты (не проверены на устройстве):
  короткий/отсутствующий ключ в необычной ветке отправителя, либо включённый
  mirror-путь AAC-ELD без `ekey`/`eiv`.
- Отказ на коротком ключе — сознательно строже S: без 32-байтового ключа
  ChaCha20-Poly1305 поток нечем расшифровывать, притворяться рабочим нельзя.
- NTP-обмен проверен на loopback; iPhone, Apple TV и звук — отдельные
  аппаратные проверки.
