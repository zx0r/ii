//! Zero-allocation `&[u8]` JSON navigator with sound lifetimes.

use memchr::memchr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nav<'a> {
    src: &'a [u8],
    pos: u32,
    end: u32,
}

#[derive(Clone, Debug)]
pub struct Items<'a> {
    src: &'a [u8],
    pos: u32,
    end: u32,
}

#[inline]
const fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

#[inline]
const fn skip_ws(src: &[u8], mut p: usize, end: usize) -> usize {
    while p < end && is_ws(src[p]) {
        p += 1;
    }
    p
}

/// Advance past a JSON string starting at `p` (where `src[p] == b'"'`).
/// Returns index one byte past closing `"` or `None` on malformed input.
#[inline]
fn skip_str(src: &[u8], p: usize, end: usize) -> Option<usize> {
    if p >= end || src[p] != b'"' {
        return None;
    }
    let mut cur = p + 1;
    while cur < end {
        let rem = &src[cur..end];
        let off = memchr::memchr2(b'"', b'\\', rem)?;
        cur += off;
        if src[cur] == b'"' {
            return Some(cur + 1);
        }
        cur += 2;
    }
    None
}

/// Advance past one JSON value starting at `p` (already whitespace-skipped).
/// Returns `(val_start, val_end)`.
fn skip_val(src: &[u8], p: usize, end: usize) -> Option<(usize, usize)> {
    let start = skip_ws(src, p, end);
    if start >= end {
        return None;
    }
    match src[start] {
        b'"' => {
            let stop = skip_str(src, start, end)?;
            Some((start, stop))
        }
        b'{' | b'[' => {
            let open = src[start];
            let close = if open == b'{' { b'}' } else { b']' };
            let mut depth = 1u32;
            let mut cur = start + 1;
            while cur < end {
                match src[cur] {
                    b'"' => cur = skip_str(src, cur, end)?,
                    b if b == open => {
                        depth += 1;
                        cur += 1;
                    }
                    b if b == close => {
                        depth -= 1;
                        cur += 1;
                        if depth == 0 {
                            return Some((start, cur));
                        }
                    }
                    _ => cur += 1,
                }
            }
            None
        }
        _ => {
            let mut cur = start;
            while cur < end && !is_ws(src[cur]) && !matches!(src[cur], b',' | b'}' | b']') {
                cur += 1;
            }
            if cur > start {
                Some((start, cur))
            } else {
                None
            }
        }
    }
}


impl<'a> Nav<'a> {
    #[inline]
    #[must_use]
    pub const fn new(src: &'a [u8]) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "input payload bounded to <= 64 MiB"
        )]
        let len = if src.len() > u32::MAX as usize {
            u32::MAX
        } else {
            src.len() as u32
        };
        Self {
            src,
            pos: 0,
            end: len,
        }
    }

    #[inline]
    #[must_use]
    pub const fn span(&self) -> (u32, u32) {
        (self.pos, self.end)
    }

    #[inline]
    #[must_use]
    pub fn raw(&self) -> &'a [u8] {
        let p = skip_ws(self.src, self.pos as usize, self.end as usize);
        let mut e = self.end as usize;
        while e > p && is_ws(self.src[e - 1]) {
            e -= 1;
        }
        &self.src[p..e]
    }

    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<Self> {
        let end = self.end as usize;
        let mut cur = skip_ws(self.src, self.pos as usize, end);
        if cur >= end || self.src[cur] != b'{' {
            return None;
        }
        cur += 1;
        loop {
            cur = skip_ws(self.src, cur, end);
            if cur >= end || self.src[cur] == b'}' {
                return None;
            }
            if self.src[cur] == b',' {
                cur = skip_ws(self.src, cur + 1, end);
            }
            if cur >= end || self.src[cur] != b'"' {
                return None;
            }
            let k_end = skip_str(self.src, cur, end)?;
            let k_bytes = &self.src[cur + 1..k_end - 1];
            cur = skip_ws(self.src, k_end, end);
            if cur >= end || self.src[cur] != b':' {
                return None;
            }
            let (v_start, v_end) = skip_val(self.src, cur + 1, end)?;
            if k_bytes == key {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "v_start and v_end are <= self.end which is u32"
                )]
                return Some(Self {
                    src: self.src,
                    pos: v_start as u32,
                    end: v_end as u32,
                });
            }
            cur = v_end;
        }
    }

    #[must_use]
    pub const fn items(&self) -> Items<'a> {
        let end = self.end as usize;
        let cur = skip_ws(self.src, self.pos as usize, end);
        if cur < end && self.src[cur] == b'[' {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "cur + 1 <= self.end which is u32"
            )]
            Items {
                src: self.src,
                pos: (cur + 1) as u32,
                end: self.end,
            }
        } else {
            Items {
                src: self.src,
                pos: self.end,
                end: self.end,
            }
        }
    }

    #[must_use]
    pub fn str_raw(&self) -> Option<&'a [u8]> {
        let end = self.end as usize;
        let start = skip_ws(self.src, self.pos as usize, end);
        if start >= end || self.src[start] != b'"' {
            return None;
        }
        let stop = skip_str(self.src, start, end)?;
        Some(&self.src[start + 1..stop - 1])
    }

    /// Zero-copy fast path when string contains no backslash escapes;
    /// otherwise unescapes into `scratch` and returns a slice tied to `'b`.
    pub fn str_of<'b>(&self, scratch: &'b mut Vec<u8>) -> Option<&'b [u8]>
    where
        'a: 'b,
    {
        let raw = self.str_raw()?;
        if memchr(b'\\', raw).is_none() {
            return Some(raw);
        }
        let base = scratch.len();
        let mut i = 0;
        while i < raw.len() {
            if raw[i] != b'\\' {
                scratch.push(raw[i]);
                i += 1;
                continue;
            }
            i += 1;
            if i >= raw.len() {
                break;
            }
            match raw[i] {
                b'"' | b'\\' | b'/' => {
                    scratch.push(raw[i]);
                    i += 1;
                }
                b'n' => {
                    scratch.push(b'\n');
                    i += 1;
                }
                b'r' => {
                    scratch.push(b'\r');
                    i += 1;
                }
                b't' => {
                    scratch.push(b'\t');
                    i += 1;
                }
                b'b' => {
                    scratch.push(0x08);
                    i += 1;
                }
                b'f' => {
                    scratch.push(0x0C);
                    i += 1;
                }
                b'u' if i + 4 < raw.len() => {
                    let h0 = crate::theme::hex_digit(raw[i + 1])?;
                    let h1 = crate::theme::hex_digit(raw[i + 2])?;
                    let h2 = crate::theme::hex_digit(raw[i + 3])?;
                    let h3 = crate::theme::hex_digit(raw[i + 4])?;
                    let cp = (u32::from(h0) << 12)
                        | (u32::from(h1) << 8)
                        | (u32::from(h2) << 4)
                        | u32::from(h3);
                    if let Some(ch) = char::from_u32(cp) {
                        let mut u8_buf = [0u8; 4];
                        scratch.extend_from_slice(ch.encode_utf8(&mut u8_buf).as_bytes());
                    }
                    i += 5;
                }
                other => {
                    scratch.push(other);
                    i += 1;
                }
            }
        }
        Some(&scratch[base..])
    }

    #[must_use]
    pub fn num_of(&self) -> Option<f64> {
        let s = core::str::from_utf8(self.raw()).ok()?;
        s.parse::<f64>().ok()
    }

    #[must_use]
    pub fn int_of(&self) -> Option<i64> {
        let s = core::str::from_utf8(self.raw()).ok()?;
        s.parse::<i64>().ok()
    }
}

impl<'a> Iterator for Items<'a> {
    type Item = Nav<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let end = self.end as usize;
        let mut cur = skip_ws(self.src, self.pos as usize, end);
        if cur < end && self.src[cur] == b',' {
            cur = skip_ws(self.src, cur + 1, end);
        }
        if cur >= end || self.src[cur] == b']' {
            self.pos = self.end;
            return None;
        }
        let Some((v_start, v_end)) = skip_val(self.src, cur, end) else {
            self.pos = self.end;
            return None;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "v_start and v_end are <= self.end which is u32"
        )]
        {
            self.pos = v_end as u32;
            Some(Nav {
                src: self.src,
                pos: v_start as u32,
                end: v_end as u32,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Nav;

    #[test]
    fn nav_nested_and_str_lifetimes() {
        let doc = br#"{"mark":"bar","data":{"values":[{"x":"A\nB","y":42.5},{"x":"C","y":-7}]}}"#;
        let root = Nav::new(doc);
        let mut scratch = Vec::new();
        assert_eq!(
            root.get(b"mark").and_then(|n| n.str_raw()),
            Some(&b"bar"[..])
        );
        let vals = root
            .get(b"data")
            .and_then(|d| d.get(b"values"))
            .map(|v| v.items().collect::<Vec<_>>())
            .unwrap_or_default();
        assert_eq!(vals.len(), 2);
        assert_eq!(
            vals[0].get(b"x").and_then(|n| n.str_of(&mut scratch)),
            Some(&b"A\nB"[..])
        );
        assert_eq!(vals[0].get(b"y").and_then(|n| n.num_of()), Some(42.5));
        assert_eq!(vals[1].get(b"y").and_then(|n| n.int_of()), Some(-7));
    }
}
