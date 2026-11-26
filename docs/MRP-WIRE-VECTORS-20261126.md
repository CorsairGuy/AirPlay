# Независимые wire-векторы MRP

26 ноября 2026. Ветка `feat/mrp-wire-vectors` от `origin/main` `b612eb4`. Задача P1 MRP — «нужны независимые векторы».

Аппаратной проверки нет: iPhone и Apple TV недоступны, живой MRP-сеанс не снимался.

## Что сделано

`crates/mrp/tests/wire_vectors.rs` — 8 тестов на байтах, написанных руками по кодировке protobuf (tag = `field << 3 | type`, varint, little-endian fixed32/fixed64). Ни один вектор не построен нашим encoder'ом, поэтому ошибка кодирования не может «согласоваться» сама с собой.

| Вектор | Что проверяет |
| --- | --- |
| `envelope_fields_parse_from_hand_written_bytes` | `08 0c 12 03 61 62 63` → поле 1 = 12, поле 2 = `"abc"` |
| `multi_byte_varints_tags_and_boundaries` | поле 16 через двухбайтовый tag, значение 300; повторяющееся поле сохраняет обе записи и `get` отдаёт последнюю; `u64::MAX` |
| `fixed_fields_keep_their_bit_pattern` | fixed64 = f64 12.5 и fixed32 = f32 0.5 без потери битов |
| `nested_message_is_length_delimited` | поле 3 длиной 2 с вложенным `08 2a`; `raw` отдаёт ровно те байты |
| `malformed_bytes_are_rejected_explicitly` | поле 0, wire type 3 и 4, короткие fixed64/fixed32, длина больше буфера, короткий varint, varint длиннее 64 бит |
| `packet_framing_uses_varint_length_prefixes` | `02 08 0c 03 08 cc 01` → два сообщения, и `encode_packet` воспроизводит те же байты |
| `packet_without_a_length_prefix_is_accepted_once` | одно сообщение, начинающееся с тега `type` (`0x08`), как шлют некоторые версии iOS |
| `packet_framing_rejects_zero_truncated_and_missing_length` | нулевая длина, обрезанное тело, пустое тело; пустой пакет даёт пустой список |

## Чего эти векторы не доказывают

- Это разбор формата, а не золотые сообщения D55: `SET_STATE`, `DeviceInfo`, topology, `MODIFY_OUTPUT_CONTEXT_REQUEST` и `PLAYBACK_QUEUE_REQUEST` проверяются на уровне `Node` и `model::state`, но независимых байтовых векторов для каждого из них ещё нет.
- Не проверяются protobuf-группы (wire type 3/4) — они намеренно отвергаются, и packed-повторения: наш разбор их не поддерживает.
- Поведение реального iPhone (подписка, переподключение, обложка, команды) не проверялось — аппаратная проверка открыта.

## Проверка

```
cargo test --offline -p mrp        14 lib + 8 wire_vectors passed
cargo clippy --offline -p mrp --all-targets -- -D warnings   ok
cargo fmt -p mrp -- --check        замечания только в src/wire.rs (не мои правки)
```

## Замечание по ходу

Повторяющееся поле в `Node` хранит все записи, но `get`/`number` возвращают последнюю — это совпадает с поведением protobuf для неповторяющихся полей и теперь зафиксировано тестом. Если понадобится первая или все записи, это отдельное API, а не смена текущего.
