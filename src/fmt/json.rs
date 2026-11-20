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
    let trimmed = raw.trim_ascii();
    if trimmed.len() < 2 || trimmed[0] != b'"' || trimmed[trimmed.len() - 1] != b'"' {
        return false;
    }
    let body = &trimmed[1..trimmed.len() - 1];
    out.reserve(body.len());
    let mut pos = 0usize;
    while pos < body.len() {
        let Some(off) = memchr::memchr(b'\\', &body[pos..]) else {
            out.extend_from_slice(&body[pos..]);
            return true;
        };
        out.extend_from_slice(&body[pos..pos + off]);
        pos += off + 1;
        let Some(&esc) = body.get(pos) else {
            return false;
        };
        match esc {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0C),
            b'u' => {
                if let Some(hex) = body.get(pos + 1..pos + 5)
                    && let Ok(s) = core::str::from_utf8(hex)
                    && let Ok(code) = u32::from_str_radix(s, 16)
                    && let Some(ch) = char::from_u32(code)
                {
                    let mut utf8 = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
                }
                pos += 4;
            }
            _ => out.push(esc),
        }
        pos += 1;
    }
    true
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
