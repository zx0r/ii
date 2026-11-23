//! Zero-copy single-pass JSON byte-slice navigator with bounded nesting depth (`MAX_DEPTH = 64`).

const MAX_DEPTH: u8 = 64;

#[inline]
#[must_use]
pub fn skip_ws(src: &[u8], mut pos: usize) -> usize {
    while let Some(&byte) = src.get(pos) {
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
            pos += 1;
        } else {
            break;
        }
    }
    pos
}

#[inline]
#[must_use]
pub fn skip_string(src: &[u8], pos: usize) -> Option<usize> {
    if src.get(pos) != Some(&b'"') {
        return None;
    }
    let mut cur = pos + 1;
    while cur < src.len() {
        let off = memchr::memchr2(b'"', b'\\', &src[cur..])?;
        cur += off;
        if src[cur] == b'"' {
            return Some(cur + 1);
        }
        cur = cur.checked_add(2)?;
    }
    None
}

#[must_use]
pub fn skip_value(src: &[u8], start: usize, depth: u8) -> Option<usize> {
    if depth >= MAX_DEPTH {
        return None;
    }
    let pos = skip_ws(src, start);
    match *src.get(pos)? {
        b'"' => skip_string(src, pos),
        b'{' => {
            let mut cur = skip_ws(src, pos + 1);
            if src.get(cur) == Some(&b'}') {
                return Some(cur + 1);
            }
            loop {
                cur = skip_string(src, cur)?;
                cur = skip_ws(src, cur);
                if src.get(cur) != Some(&b':') {
                    return None;
                }
                cur = skip_value(src, cur + 1, depth + 1)?;
                cur = skip_ws(src, cur);
                match *src.get(cur)? {
                    b',' => cur = skip_ws(src, cur + 1),
                    b'}' => return Some(cur + 1),
                    _ => return None,
                }
            }
        }
        b'[' => {
            let mut cur = skip_ws(src, pos + 1);
            if src.get(cur) == Some(&b']') {
                return Some(cur + 1);
            }
            loop {
                cur = skip_value(src, cur, depth + 1)?;
                cur = skip_ws(src, cur);
                match *src.get(cur)? {
                    b',' => cur = skip_ws(src, cur + 1),
                    b']' => return Some(cur + 1),
                    _ => return None,
                }
            }
        }
        _ => {
            let mut cur = pos;
            while let Some(&byte) = src.get(cur) {
                if matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                    break;
                }
                cur += 1;
            }
            (cur > pos).then_some(cur)
        }
    }
}

/// Lookup raw byte slice of field `key` in a JSON object without allocating.
#[must_use]
pub fn get_field<'a>(obj: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let pos = skip_ws(obj, 0);
    if obj.get(pos) != Some(&b'{') {
        return None;
    }
    let mut cur = skip_ws(obj, pos + 1);
    if obj.get(cur) == Some(&b'}') {
        return None;
    }
    loop {
        let key_end = skip_string(obj, cur)?;
        let raw_key = &obj[cur + 1..key_end - 1];
        cur = skip_ws(obj, key_end);
        if obj.get(cur) != Some(&b':') {
            return None;
        }
        let val_start = skip_ws(obj, cur + 1);
        let val_end = skip_value(obj, val_start, 1)?;
        if raw_key == key {
            return Some(&obj[val_start..val_end]);
        }
        cur = skip_ws(obj, val_end);
        match *obj.get(cur)? {
            b',' => cur = skip_ws(obj, cur + 1),
            _ => return None,
        }
    }
}

/// Zero-copy iterator over raw element slices of a JSON array.
#[derive(Debug)]
pub struct ArrayIter<'a> {
    src: &'a [u8],
    pos: usize,
    done: bool,
}

#[must_use]
pub fn iter_array(arr: &[u8]) -> ArrayIter<'_> {
    let pos = skip_ws(arr, 0);
    if arr.get(pos) != Some(&b'[') {
        return ArrayIter {
            src: arr,
            pos: arr.len(),
            done: true,
        };
    }
    let first = skip_ws(arr, pos + 1);
    let done = arr.get(first) == Some(&b']');
    ArrayIter {
        src: arr,
        pos: first,
        done,
    }
}

impl<'a> Iterator for ArrayIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let start = skip_ws(self.src, self.pos);
        let end = skip_value(self.src, start, 1)?;
        let item = &self.src[start..end];
        let after = skip_ws(self.src, end);
        match self.src.get(after) {
            Some(&b',') => self.pos = skip_ws(self.src, after + 1),
            _ => self.done = true,
        }
        Some(item)
    }
}

/// Unescape a JSON string token (`"..."`) directly into `out`.
pub fn unescape_str(raw: &[u8], out: &mut Vec<u8>) -> bool {
    let start = out.len();
    if decode_str(raw.trim_ascii(), out).is_none() {
        out.truncate(start);
        return false;
    }
    true
}

fn hex4(src: &[u8], pos: &mut usize) -> Option<u32> {
    let bytes = src.get(*pos..pos.checked_add(4)?)?;
    let mut value = 0;
    for &byte in bytes {
        value = value * 16 + u32::from(crate::theme::hex_digit(byte)?);
    }
    *pos += 4;
    Some(value)
}

fn decode_str(raw: &[u8], out: &mut Vec<u8>) -> Option<()> {
    let body = raw.strip_prefix(b"\"")?.strip_suffix(b"\"")?;
    let mut pos = 0;
    while pos < body.len() {
        let end = memchr::memchr(b'\\', &body[pos..]).map_or(body.len(), |n| pos + n);
        let plain = &body[pos..end];
        if plain.iter().any(|&b| b < 32 || b == b'"') || core::str::from_utf8(plain).is_err() {
            return None;
        }
        out.extend_from_slice(plain);
        if end == body.len() {
            return Some(());
        }
        pos = end + 1;
        let escape = *body.get(pos)?;
        pos += 1;
        match escape {
            b'"' | b'\\' | b'/' => out.push(escape),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'u' => {
                let mut code = hex4(body, &mut pos)?;
                if (0xd800..=0xdbff).contains(&code) {
                    if body.get(pos..pos + 2)? != b"\\u" {
                        return None;
                    }
                    pos += 2;
                    let low = hex4(body, &mut pos)?;
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return None;
                    }
                    code = 0x10000 + ((code - 0xd800) << 10) + low - 0xdc00;
                }
                let mut bytes = [0; 4];
                out.extend_from_slice(char::from_u32(code)?.encode_utf8(&mut bytes).as_bytes());
            }
            _ => return None,
        }
    }
    Some(())
}

#[inline]
#[must_use]
pub fn parse_f64(raw: &[u8]) -> Option<f64> {
    core::str::from_utf8(raw.trim_ascii())
        .ok()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}
