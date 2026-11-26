# Независимые PTP-векторы: Signaling/TLV, Announce, Delay_Req

Ветка `feat/ptp-wire-vectors`, база — `main` `b612eb4`, 2026-11-26.
Задача **B** из [AGENT-TASKS](AGENT-TASKS.md) (инвентаризация — ветка
`feat/station-core-inventory`).

## Что сделано

`crates/clock/tests/ptp_wire_vectors.rs` — шесть интеграционных тестов, где
**все байты записаны вручную** по IEEE 1588-2019 (clause 13) и по формату
Apple unicast-negotiation. Ни один ожидаемый пакет не сгенерирован кодом,
который тестируется, поэтому изменение энкодера не перепишет ожидание.

Покрыто:

- **Delay_Req → Delay_Resp.** Запрос 44 байта от чужого clock identity;
  проверяются: тип `0x19`, версия 2, длина 54, домен 0, флаги `0x0608`
  (unicast `0x0400`, two-step `0x0200`, ptpTimescale `0x0008`), наш
  `sourcePortIdentity`, тот же `sequenceId`, `controlField = 0`,
  `logMessageInterval = -3`, `receiveTimestamp` (секунды 6 байт + наносекунды
  `u32`) и дословное эхо `requestingPortIdentity` в 44..54.
- **Announce.** Поля читаются по документированным смещениям:
  `priority1` 47, `clockClass` 48, `accuracy` 49, `variance` 50..52,
  `priority2` 52, `grandmasterIdentity` 53..61, `stepsRemoved` 61..63,
  `sourcePortIdentity` 20..30. 76-байтовый вариант с vendor-расширением и
  чистый 64-байтовый принимаются.
- **Signaling/TLV.** Запрос 54 байта: заголовок + target identity (34..44) +
  TLV `0x0004` (REQUEST_UNICAST_TRANSMISSION) длиной 6. Проверяется грант:
  тип `0x1c`, длина 56, флаги `0x0400`, `controlField = 5`,
  `logMessageInterval = 0x7f`, target = requester, TLV `0x0005`
  (GRANT_UNICAST_TRANSMISSION) длиной 8 с парой `(messageType, interval)` и
  `duration`, плюс `renew`.
- **Ограничения политики.** Announce выдаётся только с интервалом 0, Sync —
  только с `-3` (0xfd) при 2^-3 с; `duration` урезается до 300 с; нулевая
  длительность, чужой target identity, собственный clock identity, короткий
  заголовок и TLV нечётной длины не порождают грант.

Отказы `Announce`: ненулевой домен, отсутствие transportSpecific-ниббла,
несовпадение длины в заголовке, нулевой grandmaster, нулевой порт источника,
`stepsRemoved = 255`, буфер короче 64 байт.

## Проверки

```
cargo test --offline -p clock --test ptp_wire_vectors  -> 6 passed
cargo clippy --offline --all-targets -p clock -- -D warnings  -> чисто
cargo fmt --check -p clock                            -> чисто
```

Первый прогон поймал две ошибки в самих ожиданиях, а не в коде: флаги
Delay_Resp — `0x0608`, а не `0x0600` (в `ptp_sender::FLAGS` есть бит
ptpTimescale), и 64-байтовый Announce обязан нести длину 64 в заголовке. Оба
ожидания исправлены по спецификации.

## Что осталось: захват реального обмена

Векторы доказывают соответствие реализованного энкодера/декодера
документированному формату, но **не** то, что так говорит устройство. Для
закрытия задачи B нужен захват:

```bash
# на машине, которая принимает AP2-звук от iPhone/Apple TV
sudo tcpdump -i any -n -s0 -w ptp.pcap 'udp port 319 or udp port 320'
# параллельно — SETUP с timingPeerInfo (RTSP на 7000/т.п.)
sudo tcpdump -i any -n -s0 -A -w rtsp.pcap 'tcp port 7000'
```

Что проверить в захвате: реальный интервал Sync (ожидается 125 мс), формат
Announce (длина, наличие vendor-расширения), набор TLV в Signaling, значение
`logMessageInterval` в Sync, поведение при отказе в гранте. Захват содержит
MAC-адреса и имена устройств — в репозиторий его не добавлять, только
выводы и обезличенные байтовые векторы.

## Ограничения

- Аппаратной проверки нет: ни iPhone, ни Apple TV, ни приёмников в окружении.
- Векторы описывают профиль, который реализует ядро, и не заявляют полный
  IEEE 1588 или BMCA: multicast, peer-delay вне этого профиля не проверялись.
- Битовые значения флагов взяты из спецификации; совпадение с тем, что шлёт
  iOS, должен подтвердить захват.
