# Источник и изменения

Протокольная последовательность портирована из `CorsairGuy/yandex_station_airplay`,
`src/airplay_sender/src/main.rs`, из локального checkout Claude:

`C:\Users\admin\AppData\Local\Temp\claude\C--Users-admin-Desktop-yandex-station-airplay\fe3bdba2-3b4f-4164-a49c-b977d54deb28\scratchpad\main`

- HEAD `feature/homepod-picker`: `e2bd2700be7f22305a8fdd47ce54eeaeb7385228`.
- Исходный sender/local sync draft: `cd7ed8b5626a2b8273bebdd4d9f63b40839460c1`.
- Исправление общей packet media epoch:
  `0df97db7860b510fb456215504bafcdcea672502`.
- Working-copy sender содержал незакоммиченную доработку живой громкости;
  SHA256 исходных байтов:
  `902e478edd2e140f2b43067951e370f694a76ef1661a363b6052845532b1e630`.
- Коммиты authored CorsairGuy, co-authored Claude; протокольная реализация использовала
  ранее опубликованные AirPlay/HAP/RAOP материалы. Это не заявление о независимом
  reverse engineering всех этих протоколов данным портом.

Новый код использует общие `ap_pair`, `ap_crypto`, `ap_rtsp`, `ap_rtp`, `timesync`,
а не копирует SRP/TLV/HAP/AEAD/RTSP-код исходного автономного executable.
Дополнительно включена проверка M4, отсутствовавшая в старом draft. Упаковка RTP,
nonce и retransmit перенесены в общий `rtp::PcmPacketizer`.

Общая packet epoch сохранена, но получена один раз от общих монотонных NTP-часов
и `Instant`; нет отдельного CLOCK_MONOTONIC ABI с запускаемым процессом. CLI,
stdin, filesystem volume polling и локальный player process не перенесены.
Dirty volume logic выражена через `watch<Volume>` на каждую сессию, а не каталог
`/run`. Identity задаёт приложение. Отсутствуют firmware names, адреса настоящих
устройств, credentials, содержимое track keys или URLs.

SHK сохраняет исходное значение HKDF **Events-Write-Encryption-Key**; в текущем
общем API это `ControlKeys::accessory_events(shared).write`, а не
`controller_events(shared).write` (последний меняет направление и равен Events-Read).
Канал событий использует `controller_events` целиком. В HANDOFF эти два направления
были перепутаны; здесь они разделены явным комментарием в коде.

## Незавершённый streaming-reader ABI исходной работы

В старом scratch HEAD `outputs.rs` уже передавал reader ключ первой stdin-строкой
и затем encrypted HTTP body, а committed `content_reader.c` ещё ожидал четыре
аргумента с именами файлов и offset. Совместимая версия reader была **dirty**:
`src/group_helpers/content_reader.c`, SHA256
`c357ac70a10e10f8f14e50516e2cea7be92d34125971174dba6c866c7bb7af3f`.

Ни reader, ни HTTP/Yandex source adapter в эту библиотеку не включены. Получатель
API подаёт уже декодированный S16BE44100 stereo PCM. Не следует переносить старый
outputs/reader по отдельности или считать эту несовместимость исправленной самим
добавлением sender. Строгая классификация decoder EOF/error также относится к
source adapter, не к сетевой сессии.

Применяется metadata лицензии текущего workspace (`GPL-3.0-or-later`); исходные
уведомления/лицензии общих зависимостей сохраняются в их компонентах. Порт не
перелицензирует сторонние компоненты и не включает firmware или новый FairPlay SAP.
