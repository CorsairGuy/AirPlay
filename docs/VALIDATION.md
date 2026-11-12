# Проверки и границы подтверждений

Обновлено12ноября2026. Текущий код и ограничения — [STATUS](STATUS.md).

| Уровень | Подтверждено | Не следует из результата |
|---|---|---|
| Unit / loopback | 311 библиотечных тестов workspace прошли на Pi/Linux, offline, без сети контейнера | Полный cargo test с binaries/integration/doctest; все ОС/feature sets; вся GitHub CI |
| RTSP/RTP mock | Pairing/encrypted RTSP, SETPEERS, d7, UDP во время задержанного ACK, EOF/cleanup | Реальное принятие группы HomePod/Apple TV, слышимый звук |
| PTP lifecycle | Loopback/ephemeral ports; общий source, role/epoch/freshness, union owners и release | LAN PTP/BMCA completeness, follower TX servo, акустический drift |
| Карточка | Wire/replay, artwork/timeline и worker regressions | Исправление всех iPhone UI/ползунков/подписи группы |
| Станционный код | Срез4ead45f:137 Rust tests + C focus-state check | Установка этого среза и сохранность звука на физической Станции |

Локальные журналы координатора: CORE-MAIN-MERGE-CHECK-20261111.log и STATION-MAIN-MERGE-CHECK-20261111.log. Они не встроены в релиз. Исторический разовый сбой PCM loopback зарегистрирован в PTP-LOCAL-CHECK-20261111.log; повтор без изменения сборки и последний workspace прогон прошли.

Рекомендуемые проверки разработки: cargo fmt --all --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace. Эти команды являются требованиями CI, а не заявлением, что полный набор уже выполнен12ноября. FFmpeg и целевые ОС/архитектуры проверяются отдельно.

Стенд не должен запускать LAN discovery, PTP319/320 или аудио устройства автоматически. Для физической проверки требуется отдельное разрешение; сохраняются откат и отдельные сценарии Alice/AirPlay/BT. Контролировать память/rootfs, процессы и восстановление маршрута, а не только наличие UI-галочки.

Последующее дополнение12ноября: source continuous policy —68 sender library tests passed, включая5 новых случаев. Полный workspace311 выше относится к предшествующему исходному срезу; включённая continuous-сессия и аппаратный interop этим не подтверждены.

Continuous async source pump:73 sender library tests passed, включая5 новых async-сценариев; только память, bounded-каналы и таймеры. Сетевой continuous runtime и установленная Станция этим не подтверждены.

Continuous broadcast/revision guards:78 sender library tests passed; дополнили encrypted-loopback и local renderer проверки. Hardware/receiver buffers и сетевой pause/seek автомат остаются отдельной незавершённой работой.

Continuous retransmit protection:94 tests passed (16 RTP,78 sender), включая cache clear без сброса nonce/sequence/media epoch. Это не проверка remote PAUSE/FLUSH или hardware buffer reset.
