#!/usr/bin/env python3
"""Генерирует crates/mrp/src/schema.rs из proto/*.proto.

    python3 crates/mrp/tools/schema.py

proto/ — описания MRP из pyatv (pyatv/protocols/mrp/protobuf, MIT,
proto/LICENSE-pyatv.md), из них же собрана схема airplay_mrp на станции.
Тело сообщения типа KIND_NAME лежит в расширении ProtocolMessage kindName.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PROTO = ROOT / "proto"
OUT = ROOT / "src" / "schema.rs"

KEYWORDS = {
    "as", "break", "const", "continue", "crate", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "static", "struct", "super",
    "trait", "true", "type", "unsafe", "use", "where", "while", "async",
    "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
}
VARINT = {"bool", "int32", "int64", "uint32", "uint64", "sint32", "sint64"}
FIXED64 = {"double", "fixed64", "sfixed64"}
FIXED32 = {"float", "fixed32", "sfixed32"}
BYTES = {"string", "bytes"}
# Типы, тело которых лежит в чужом расширении (как в pyatv messages.py).
SHARED = {"DEVICE_INFO_UPDATE_MESSAGE": "deviceinfomessage"}


def ident(name):
    return f"r#{name}" if name in KEYWORDS else name


def parse(text, messages, enums, nested):
    text = re.sub(r"//[^\n]*", "", text)
    text = re.sub(r'"[^"]*"', "", text)
    tokens = re.findall(r"[A-Za-z_][A-Za-z0-9_.]*|-?\d+|[{};=]", text)
    stack = []
    i = 0
    while i < len(tokens):
        t = tokens[i]
        if t == "extend":
            stack.append(("message", tokens[i + 1]))
            messages.setdefault(tokens[i + 1], [])
            i += 3
        elif t in ("message", "enum"):
            name = tokens[i + 1]
            outer = stack[-1][1] if stack else None
            if t == "message":
                stack.append(("message", name))
                messages.setdefault(name, [])
            else:
                # Перечисление внутри сообщения X — значения в value::X.
                full = outer or name
                if outer:
                    nested.add(f"{outer}.{name}")
                stack.append(("enum", full, outer, name))
                enums.setdefault(full, [])
            i += 3
        elif t == "}":
            stack.pop()
            i += 1
        elif t in ("optional", "repeated", "required"):
            typ, name, number = tokens[i + 1], tokens[i + 2], int(tokens[i + 4])
            messages[stack[-1][1]].append((name, typ, number))
            i += 6
        elif stack and stack[-1][0] == "enum" and tokens[i + 1] == "=":
            enums[stack[-1][1]].append((t, int(tokens[i + 2])))
            i += 4
        elif t in ("syntax", "package", "import", "extensions", "reserved", "option"):
            i = tokens.index(";", i) + 1
        else:
            sys.exit(f"не разобрать proto у «{t}»")


def wire(typ, owner, messages, enums, nested):
    if typ in VARINT or typ.endswith(".Enum") or f"{owner}.{typ}" in nested:
        return 0, ""
    if typ in FIXED64:
        return 1, ""
    if typ in FIXED32:
        return 5, ""
    if typ in BYTES:
        return 2, ""
    if typ in messages:
        return 2, typ
    if typ in enums:
        return 0, ""
    sys.exit(f"неизвестный тип {typ} в {owner}")


def main():
    messages, enums, nested = {}, {}, set()
    for path in sorted(PROTO.glob("*.proto")):
        parse(path.read_text(), messages, enums, nested)
    # Типы ProtocolMessage идут в kind::, остальное — в value::.
    kinds = enums.pop("ProtocolMessage")
    out = [
        "// Сгенерировано crates/mrp/tools/schema.py из proto/*.proto: не править руками.",
        "#![allow(non_upper_case_globals, non_snake_case)]",
        "",
        "/// Номера полей: `field::DeviceInfoMessage::deviceUID`.",
        "pub mod field {",
    ]
    for name in sorted(messages):
        fields = messages[name]
        if not fields:
            continue
        out.append(f"    pub mod {name} {{")
        for fname, _, number in fields:
            out.append(f"        pub const {ident(fname)}: u32 = {number};")
        out.append("    }")
    out += ["}", "", "/// Типы `ProtocolMessage`.", "pub mod kind {"]
    for name, number in kinds:
        out.append(f"    pub const {name}: u32 = {number};")
    out.append("}")

    ext = {f.lower(): n for f, _, n in messages["ProtocolMessage"]}
    out += [
        "",
        "/// Поле `ProtocolMessage`, в котором лежит тело сообщения этого типа.",
        "pub fn extension(kind: u32) -> Option<u32> {",
        "    match kind {",
    ]
    for name, number in kinds:
        key = SHARED.get(name) or name.replace("_", "").lower()
        if key in ext:
            out.append(f"        {number} => Some({ext[key]}),")
    out += ["        _ => None,", "    }", "}", "",
            "/// Значения перечислений: `value::SendError::NotSupported`.",
            "pub mod value {"]
    for name in sorted(enums):
        out.append(f"    pub mod {name} {{")
        for vname, number in enums[name]:
            out.append(f"        pub const {ident(vname)}: u64 = {number};")
        out.append("    }")
    out += [
        "}",
        "",
        "/// Тип поля по проводу (0 varint, 1 fixed64, 2 длина, 5 fixed32) и",
        "/// вложенное сообщение: для проверки того, что собрано.",
        "pub fn wire_type(message: &str, field: u32) -> Option<(u8, &'static str)> {",
        "    match (message, field) {",
    ]
    for name in sorted(messages):
        for _, typ, number in messages[name]:
            w, inner = wire(typ, name, messages, enums, nested)
            out.append(f'        ("{name}", {number}) => Some(({w}, "{inner}")),')
    out += ["        _ => None,", "    }", "}", ""]
    OUT.write_text("\n".join(out))


if __name__ == "__main__":
    main()
