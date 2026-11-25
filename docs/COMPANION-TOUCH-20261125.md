# Companion: тачпад `_hidT` — 25 ноября 2026

Ветка `feat/companion-touch` от `feat/companion-remote` (`ba9b91c`):
слой Companion уже содержит конверты, OPACK и HID-кнопки, здесь добавлен
тачпад. В `main` пакета `companion` нет, поэтому ветка стековая.

## Источники wire-фактов

- pyatv, `pyatv/protocols/companion/api.py` (`hid_event`, `swipe`, `action`)
  и `docs/documentation/protocols.md` — захваты запросов `_hidC`/`_hidT`.
- `jxgedouard/remote-me`, `docs/apple-remote-protocol.md` — `_touchStart`
  (`{_width, _height, _tFl}`) и `_touchStop`.

Код референсов не портировался: взяты имена полей, единицы и порядок
событий.

## Что сделано (`crates/companion/src/commands.rs`, `link.rs`)

- **`TOUCHPAD_MAX = 1000`** — обе оси, начало в левом верхнем углу.
- **`TouchPhase`** — `Press = 1`, `Hold = 3`, `Release = 4`, `Click = 5`.
- **`touch_start(xid)`** — `_touchStart` с `_width`/`_height` как `Double`
  1000.0 и `_tFl = 0`; **`touch_stop(xid)`** — `_touchStop`.
- **`touch(x, y, phase, ns, xid)`** — событие `_hidT`: конверт `_t = 1`
  (event), но с `_x`, как в захватах pyatv; `_c = {_ns, _tFg: 1, _tPh,
  _cx, _cy}`; координаты прижимаются к `0..=1000`, а не отбрасываются.
- **`tap(xid, ns)`** — `_hidC` Select down, Select up и `_hidT` `Click`
  в (1000, 1000); три значения с последовательными `_x`.
- **`swipe_points(from, to, steps)`** — равномерные точки свайпа с
  сохранением обоих концов (интерполяция вызывающего, без задержек).
- **`Link::touch(...)`** — выдаёт новый `_x`, шлёт один шифрованный
  OPACK-кадр и возвращает его байты.
- `_ns` — наносекунды с момента установления соединения (так его считает
  pyatv от `_base_timestamp`); вызывающий передаёт значение сам.

## Проверки

```
cargo test --offline -p companion     # 34 passed (было 27)
cargo clippy --offline -p companion --all-targets -- -D warnings
cargo fmt -p companion -- --check
```

Новые тесты (7): поля и `_t`/`_x` события, значения фаз, прижатие
координат, `_touchStart`/`_touchStop`, последовательность тапа,
концы свайпа, кадр `Link::touch` (тип кадра, длина, OPACK-roundtrip).

## Границы

- Текстовый ввод (RTI, `_tiStart`/`_tiC` с NSKeyedArchiver) **не сделан**:
  без захвата архива полей не угадываем; в `commands` пока только
  `text_input_start`/`text_input_stop`.
- Ответ `_touchStart` (идентификатор устройства касания) не разбирается —
  поле в захватах названо косвенно.
- Свайп — только точки; темп (~20 мс на событие) и удержание задаёт
  вызывающий.
- Аппаратной проверки нет: ни один жест не отправлялся на Apple TV;
  проверены только кодирование и roundtrip OPACK/кадра.
