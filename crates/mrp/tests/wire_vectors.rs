//! Независимые wire-векторы MRP.
//!
//! Байты ниже написаны руками по кодировке protobuf (tag = field << 3 | type,
//! varint, little-endian fixed32/fixed64), а не получены нашим encoder'ом:
//! тест не сможет «согласоваться» с ошибкой кодирования, которую оба конца
//! делают одинаково. Векторы покрывают то, что нужно разбору входящих
//! сообщений MRP, а не полный `protobuf` (группы, packed, unknown wire types).
//!
//! Аппаратного захвата с iPhone нет: это разбор формата, а не подтверждение
//! поведения устройства.

use mrp::wire::{Node, Value, decode_packet, encode_packet};

/// Разбор hex-строки вида `"08 0c 12 03 61 62 63"`.
fn bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for pair in text.split_whitespace() {
        assert_eq!(pair.len(), 2, "пара hex: {pair}");
        out.push(u8::from_str_radix(pair, 16).expect("hex"));
    }
    out
}

#[test]
fn envelope_fields_parse_from_hand_written_bytes() {
    // type(1, varint)=12, identifier(2, bytes)="abc"
    let node = Node::parse(&bytes("08 0c 12 03 61 62 63")).unwrap();
    assert_eq!(node.number(1), Some(12));
    assert_eq!(node.string(2), Some("abc"));
    assert_eq!(node.0.len(), 2);
}

#[test]
fn multi_byte_varints_tags_and_boundaries() {
    // Поле 16 (tag 0x80 0x01), значение 300 (0xac 0x02).
    let node = Node::parse(&bytes("80 01 ac 02")).unwrap();
    assert_eq!(node.number(16), Some(300));
    // Повторяющееся поле сохраняет обе записи в порядке провода, а `get`
    // отдаёт последнюю — как protobuf для неповторяющегося поля.
    let repeated = Node::parse(&bytes("08 01 08 02")).unwrap();
    assert_eq!(repeated.0.len(), 2);
    assert_eq!(repeated.0[0], (1, Value::Int(1)));
    assert_eq!(repeated.number(1), Some(2));
    // u64::MAX = ff ff ff ff ff ff ff ff ff 01.
    let max = Node::parse(&bytes("08 ff ff ff ff ff ff ff ff ff 01")).unwrap();
    assert_eq!(max.number(1), Some(u64::MAX));
}

#[test]
fn fixed_fields_keep_their_bit_pattern() {
    // field 1 fixed64 = f64 12.5 (0x4029000000000000 little-endian).
    let double = Node::parse(&bytes("09 00 00 00 00 00 00 29 40")).unwrap();
    assert_eq!(double.real(1), Some(12.5));
    assert_eq!(double.get(1), Some(&Value::Fixed64(0x4029_0000_0000_0000)));
    // field 2 fixed32 = f32 0.5 (0x3f000000 little-endian).
    let single = Node::parse(&bytes("15 00 00 00 3f")).unwrap();
    assert_eq!(single.real(2), Some(0.5));
    assert_eq!(single.get(2), Some(&Value::Fixed32(0x3f00_0000)));
}

#[test]
fn nested_message_is_length_delimited() {
    // field 3 (bytes) длиной 2: внутри field 1 varint 42.
    let node = Node::parse(&bytes("1a 02 08 2a")).unwrap();
    let inner = node.nested(3).unwrap();
    assert_eq!(inner.number(1), Some(42));
    assert_eq!(node.raw(3), Some(&bytes("08 2a")[..]));
}

#[test]
fn malformed_bytes_are_rejected_explicitly() {
    for (name, hex) in [
        ("поле с номером 0", "00"),
        ("неизвестный тип поля (группа)", "0b"),
        ("неизвестный тип поля (4)", "0c"),
        ("короткий fixed64", "09 00 00"),
        ("короткий fixed32", "15 00"),
        ("длина поля больше буфера", "12 05 61 62"),
        ("короткий varint", "08 80"),
        ("varint больше 64 бит", "08 ff ff ff ff ff ff ff ff ff 02"),
    ] {
        assert!(
            Node::parse(&bytes(hex)).is_err(),
            "{name}: разбор обязан отказать"
        );
    }
}

#[test]
fn packet_framing_uses_varint_length_prefixes() {
    // Два сообщения: {field 1 = 12} и {field 1 = 204}.
    let packet = bytes("02 08 0c 03 08 cc 01");
    let messages = decode_packet(&packet).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].number(1), Some(12));
    assert_eq!(messages[1].number(1), Some(204));
    // Наш пакет, собранный вручную, обязан совпасть с encode_packet.
    assert_eq!(encode_packet(&messages), packet);
}

#[test]
fn packet_without_a_length_prefix_is_accepted_once() {
    // Некоторые версии iOS шлют одно сообщение сразу с тега `type` (0x08).
    let messages = decode_packet(&bytes("08 0c 12 00")).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].number(1), Some(12));
    assert_eq!(messages[0].string(2), Some(""));
}

#[test]
fn packet_framing_rejects_zero_truncated_and_missing_length() {
    for (name, hex) in [
        ("нулевая длина", "00"),
        ("обрезанная длина", "05 08 0c"),
        ("длина без тела", "01"),
    ] {
        assert!(decode_packet(&bytes(hex)).is_err(), "{name}");
    }
    assert!(decode_packet(&[]).unwrap().is_empty());
}
