//! Разбор plist из тела RTSP с ограничением вложенности.
//!
//! `plist::from_bytes` рекурсивен: `Value` из вложенных массивов строится и
//! уничтожается рекурсивно, и тело на 15 КБ с тысячей вложенных `<array>`
//! снимает стек потока (проверено). В приёмнике такое тело приходит из сети,
//! поэтому до разбора считаем глубину контейнеров: XML-теги или, для
//! `bplist00`, ссылки в таблице объектов. Глубина сверх [`MAX_DEPTH`] —
//! отказ без вызова парсера.
//!
//! Счётчики и смещения бинарного plist проверяются на границы, обход
//! контейнеров идёт явным стеком с запоминанием достигнутой глубины, так
//! что циклические и «размножающиеся» ссылки не взрывают время разбора.

/// Предел вложенности контейнеров (`array`, `dict`/`set`).
pub const MAX_DEPTH: usize = 64;

/// Разбирает plist, если глубина вложенности в пределах [`MAX_DEPTH`].
pub fn parse(data: &[u8]) -> Option<plist::Value> {
    if !nesting_within(data, MAX_DEPTH) {
        return None;
    }
    plist::from_bytes::<plist::Value>(data).ok()
}

/// Укладывается ли вложенность контейнеров в `limit`.
///
/// Для повреждённого ввода возвращает `true`: отказ оставляем парсеру,
/// который сообщит ошибку сам; здесь важно только не пустить его в рекурсию.
pub fn nesting_within(data: &[u8], limit: usize) -> bool {
    if data.starts_with(b"bplist") {
        binary_nesting_within(data, limit)
    } else {
        xml_nesting_within(data, limit)
    }
}

fn xml_nesting_within(data: &[u8], limit: usize) -> bool {
    let mut i = 0;
    let mut depth = 0usize;
    while i < data.len() {
        if data[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &data[i..];
        if rest.starts_with(b"<!--") {
            match find(rest, b"-->") {
                Some(end) => i += end + 3,
                None => return true,
            }
            continue;
        }
        if rest.starts_with(b"<?") || rest.starts_with(b"<!") {
            match rest.iter().position(|b| *b == b'>') {
                Some(end) => i += end + 1,
                None => return true,
            }
            continue;
        }
        let closing = rest.get(1) == Some(&b'/');
        let name_start = i + if closing { 2 } else { 1 };
        let mut name_end = name_start;
        while name_end < data.len() && is_name_byte(data[name_end]) {
            name_end += 1;
        }
        let name = &data[name_start..name_end];
        let Some(tag_end) = data[name_end..].iter().position(|b| *b == b'>') else {
            return true;
        };
        let tag_end = name_end + tag_end;
        let self_closing = tag_end > i && data[tag_end - 1] == b'/';
        if is_container(name) {
            if closing {
                depth = depth.saturating_sub(1);
            } else if !self_closing {
                depth += 1;
                if depth > limit {
                    return false;
                }
            }
        }
        i = tag_end + 1;
    }
    true
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.')
}

fn is_container(name: &[u8]) -> bool {
    name.eq_ignore_ascii_case(b"array") || name.eq_ignore_ascii_case(b"dict")
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Хвост бинарного plist: размеры смещений и ссылок, число объектов, корень
/// и начало таблицы смещений.
struct Binary<'a> {
    data: &'a [u8],
    offsets: &'a [u8],
    /// Начало таблицы смещений: объекты лежат между заголовком и ей.
    table_start: usize,
    offset_size: usize,
    ref_size: usize,
    objects: usize,
}

impl<'a> Binary<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        let trailer = data.len().checked_sub(32).map(|start| &data[start..])?;
        let offset_size = *trailer.get(6)? as usize;
        let ref_size = *trailer.get(7)? as usize;
        let objects = usize::try_from(be_u64(trailer.get(8..16)?)).ok()?;
        let table = usize::try_from(be_u64(trailer.get(24..32)?)).ok()?;
        if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) {
            return None;
        }
        if objects == 0 || table < 8 || table > data.len() {
            return None;
        }
        let table_len = objects.checked_mul(offset_size)?;
        let offsets = data.get(table..table.checked_add(table_len)?)?;
        Some(Self {
            data,
            offsets,
            table_start: table,
            offset_size,
            ref_size,
            objects,
        })
    }

    fn object_offset(&self, index: usize) -> Option<usize> {
        if index >= self.objects {
            return None;
        }
        let start = index.checked_mul(self.offset_size)?;
        let offset = be_bytes(self.offsets.get(start..start + self.offset_size)?);
        let offset = usize::try_from(offset).ok()?;
        // Объекты лежат после заголовка и до таблицы смещений.
        if offset < 8 || offset >= self.table_start {
            return None;
        }
        Some(offset)
    }

    /// Число ссылок и смещение первой из них для контейнера.
    fn references(&self, object: usize) -> Option<(usize, usize)> {
        let offset = self.object_offset(object)?;
        let marker = *self.data.get(offset)?;
        let kind = marker >> 4;
        if !matches!(kind, 0xA | 0xC | 0xD) {
            return Some((0, 0));
        }
        let low = (marker & 0x0F) as usize;
        let (count, position) = if low == 0x0F {
            let int_marker = *self.data.get(offset + 1)?;
            if int_marker >> 4 != 0x1 {
                return None;
            }
            let width = (int_marker & 0x0F) as usize + 1;
            let start = offset + 2;
            let count = usize::try_from(be_bytes(self.data.get(start..start + width)?)).ok()?;
            (count, start + width)
        } else {
            (low, offset + 1)
        };
        // Словари хранят пары «ключ, значение».
        let refs = match kind {
            0xD => count.checked_mul(2)?,
            _ => count,
        };
        let bytes = refs.checked_mul(self.ref_size)?;
        self.data.get(position..position.checked_add(bytes)?)?;
        Some((refs, position))
    }

    fn reference(&self, position: usize, index: usize) -> Option<usize> {
        let start = position.checked_add(index.checked_mul(self.ref_size)?)?;
        let value = be_bytes(self.data.get(start..start + self.ref_size)?);
        usize::try_from(value).ok()
    }
}

fn binary_nesting_within(data: &[u8], limit: usize) -> bool {
    // Битый `bplist00`: хвост или таблица объектов не сходятся. Парсеру
    // такой вход не отдаём — по его же счётчикам он мог бы аллоцировать
    // гигабайты до первой ошибки.
    let Some(binary) = Binary::new(data) else {
        return false;
    };
    // Глубина, уже достигнутая в объекте: повторные ссылки не обходятся заново.
    let mut seen = vec![0usize; binary.objects];
    let mut stack = vec![(0usize, 1usize)];
    while let Some((object, depth)) = stack.pop() {
        if object >= binary.objects || depth <= seen[object] {
            continue;
        }
        if depth > limit {
            return false;
        }
        seen[object] = depth;
        let Some((refs, position)) = binary.references(object) else {
            continue;
        };
        for index in (0..refs).rev() {
            let Some(target) = binary.reference(position, index) else {
                continue;
            };
            stack.push((target, depth + 1));
        }
    }
    true
}

fn be_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64)
}

fn be_u64(bytes: &[u8]) -> u64 {
    be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xml(depth: usize) -> Vec<u8> {
        let mut out = String::from("<?xml version=\"1.0\"?><plist version=\"1.0\">");
        for _ in 0..depth {
            out.push_str("<array>");
        }
        out.push_str("<string>x</string>");
        for _ in 0..depth {
            out.push_str("</array>");
        }
        out.push_str("</plist>");
        out.into_bytes()
    }

    /// Бинарный plist: `depth` вложенных массивов по одному элементу.
    fn binary(depth: usize, cycle: bool) -> Vec<u8> {
        let objects = depth + 1;
        let ref_size = if objects <= 0x100 {
            1
        } else if objects <= 0x1_0000 {
            2
        } else {
            4
        };
        let mut body = b"bplist00".to_vec();
        let mut offsets = Vec::with_capacity(objects);
        for index in 0..objects {
            offsets.push(body.len());
            if index == objects - 1 {
                body.push(0xA0); // пустой массив
                continue;
            }
            body.push(0xA1); // массив из одного элемента
            let target = if cycle { 0 } else { index + 1 };
            body.extend_from_slice(&target.to_be_bytes()[8 - ref_size..]);
        }
        let offset_size = if body.len() <= 0x100 {
            1
        } else if body.len() <= 0x1_0000 {
            2
        } else {
            4
        };
        let table = body.len();
        for offset in offsets {
            body.extend_from_slice(&offset.to_be_bytes()[8 - offset_size..]);
        }
        body.extend_from_slice(&[0; 6]);
        body.push(offset_size as u8);
        body.push(ref_size as u8);
        body.extend_from_slice(&(objects as u64).to_be_bytes());
        body.extend_from_slice(&0u64.to_be_bytes());
        body.extend_from_slice(&(table as u64).to_be_bytes());
        body
    }

    #[test]
    fn deep_xml_is_refused_before_parsing() {
        assert!(nesting_within(&xml(MAX_DEPTH), MAX_DEPTH));
        assert!(!nesting_within(&xml(MAX_DEPTH + 1), MAX_DEPTH));
        // Столько, сколько было в воспроизведении падения: ~15 КБ.
        assert!(!nesting_within(&xml(1000), MAX_DEPTH));
        assert!(parse(&xml(1000)).is_none());
    }

    #[test]
    fn xml_strings_and_empty_elements_do_not_add_depth() {
        let body = b"<plist><dict><key>a</key><array/><key>b</key><string>&lt;array&gt;</string></dict></plist>";
        assert!(nesting_within(body, 2));
        assert!(nesting_within(b"<plist><array/><dict/></plist>", 0));
    }

    #[test]
    fn deep_binary_is_refused_and_cycles_terminate() {
        assert!(nesting_within(&binary(MAX_DEPTH - 1, false), MAX_DEPTH));
        assert!(!nesting_within(&binary(MAX_DEPTH, false), MAX_DEPTH));
        assert!(!nesting_within(&binary(4, true), MAX_DEPTH));
        assert!(!nesting_within(&binary(1000, false), MAX_DEPTH));
    }

    #[test]
    fn ordinary_binary_plist_still_parses() {
        let mut dict = plist::Dictionary::new();
        dict.insert("rate".into(), 1.0f64.into());
        dict.insert("streams".into(), plist::Value::Array(vec![]));
        let mut body = Vec::new();
        plist::Value::Dictionary(dict)
            .to_writer_binary(&mut body)
            .unwrap();
        assert!(nesting_within(&body, MAX_DEPTH));
        assert!(parse(&body).is_some());
    }

    #[test]
    fn malformed_plists_are_left_to_the_parser() {
        assert!(parse(b"not a plist").is_none());
        assert!(!nesting_within(b"bplist00", MAX_DEPTH), "обрезанный хвост");
        let mut truncated = binary(2, false);
        truncated.truncate(20);
        assert!(!nesting_within(&truncated, MAX_DEPTH));
        assert!(parse(&truncated).is_none());

        // Счётчик объектов больше самого файла: до парсера не доходит.
        let mut lying = binary(1, false);
        let len = lying.len();
        lying[len - 24..len - 16].copy_from_slice(&(1u64 << 40).to_be_bytes());
        assert!(!nesting_within(&lying, MAX_DEPTH));
        assert!(parse(&lying).is_none());
    }
}
