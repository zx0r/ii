//! Streaming LaTeX healer: balances unclosed braces, `\left`..`\right`, and `\begin{..}` environments.

use memchr::memchr3;

/// Packed environment tags for stack-resident zero-alloc tracking.
const ENV_NAMES: &[&[u8]] = &[
    b"matrix",
    b"pmatrix",
    b"bmatrix",
    b"Bmatrix",
    b"vmatrix",
    b"Vmatrix",
    b"cases",
    b"aligned",
    b"align",
    b"align*",
    b"equation",
    b"equation*",
    b"gather",
    b"gather*",
    b"array",
    b"split",
    b"substack",
];

#[inline]
fn env_id(name: &[u8]) -> Option<u8> {
    for (idx, &cand) in ENV_NAMES.iter().enumerate() {
        if cand == name {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "ENV_NAMES has fewer than 255 entries"
            )]
            return Some(idx as u8);
        }
    }
    None
}

pub struct State {
    brace_depth: u32,
    env_stack: Vec<u8>,
    delim_stack: Vec<u8>,
}

impl State {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            brace_depth: 0,
            env_stack: Vec::new(),
            delim_stack: Vec::new(),
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.brace_depth = 0;
        self.env_stack.clear();
        self.delim_stack.clear();
    }

    /// Heal a streaming or truncated LaTeX fragment into valid compilable LaTeX inside `out`.
    pub fn heal(&mut self, fragment: &[u8], out: &mut Vec<u8>) {
        self.reset();
        out.extend_from_slice(fragment);

        // Strip trailing dangling backslash or dangling `^` / `_`
        while let Some(&last) = out.last() {
            if last.is_ascii_whitespace() {
                out.pop();
            } else {
                break;
            }
        }
        if matches!(out.last(), Some(b'^' | b'_')) {
            out.extend_from_slice(b"{}");
        }

        let len = fragment.len();
        let mut i = 0usize;
        while i < len {
            let Some(off) = memchr3(b'{', b'}', b'\\', &fragment[i..]) else {
                break;
            };
            i += off;
            match fragment[i] {
                b'{' => {
                    self.brace_depth += 1;
                    i += 1;
                }
                b'}' => {
                    self.brace_depth = self.brace_depth.saturating_sub(1);
                    i += 1;
                }
                b'\\' => {
                    if i + 1 >= len {
                        // Trailing bare `\` -> remove from `out`
                        if out.last() == Some(&b'\\') {
                            out.pop();
                        }
                        break;
                    }
                    let next = fragment[i + 1];
                    if matches!(next, b'{' | b'}' | b'\\' | b'$' | b'%' | b'&' | b'#') {
                        i += 2;
                        continue;
                    }
                    // Scan control word
                    let cmd_start = i + 1;
                    let mut cmd_end = cmd_start;
                    while cmd_end < len && fragment[cmd_end].is_ascii_alphabetic() {
                        cmd_end += 1;
                    }
                    let cmd = &fragment[cmd_start..cmd_end];
                    if cmd == b"left" {
                        let mut d = cmd_end;
                        while d < len && fragment[d].is_ascii_whitespace() {
                            d += 1;
                        }
                        if d < len {
                            self.delim_stack.push(fragment[d]);
                            i = if fragment[d] == b'\\' && d + 1 < len {
                                d + 2
                            } else {
                                d + 1
                            };
                            continue;
                        }
                    } else if cmd == b"right" {
                        let mut d = cmd_end;
                        while d < len && fragment[d].is_ascii_whitespace() {
                            d += 1;
                        }
                        self.delim_stack.pop();
                        if d < len {
                            i = if fragment[d] == b'\\' && d + 1 < len {
                                d + 2
                            } else {
                                d + 1
                            };
                            continue;
                        }
                        // Dangling `\right` at end without delimiter
                        out.push(b'.');
                    } else if cmd == b"begin" && cmd_end < len && fragment[cmd_end] == b'{' {
                        if let Some(close) = fragment[cmd_end + 1..]
                            .iter()
                            .position(|&b| b == b'}')
                        {
                            let env = &fragment[cmd_end + 1..cmd_end + 1 + close];
                            if let Some(id) = env_id(env) {
                                self.env_stack.push(id);
                            }
                            i = cmd_end + 2 + close;
                            continue;
                        }
                    } else if cmd == b"end" && cmd_end < len && fragment[cmd_end] == b'{' {
                        if let Some(close) = fragment[cmd_end + 1..]
                            .iter()
                            .position(|&b| b == b'}')
                        {
                            let env = &fragment[cmd_end + 1..cmd_end + 1 + close];
                            if let Some(id) = env_id(env) {
                                if self.env_stack.last() == Some(&id) {
                                    self.env_stack.pop();
                                }
                            }
                            i = cmd_end + 2 + close;
                            continue;
                        }
                    }
                    i = cmd_end.max(i + 1);
                }
                _ => i += 1,
            }
        }

        // Close unclosed braces first
        for _ in 0..self.brace_depth {
            out.push(b'}');
        }
        // Close unclosed `\left` delimiters with `\right.`
        for _ in 0..self.delim_stack.len() {
            out.extend_from_slice(b"\\right.");
        }
        // Close unclosed `\begin{...}` in LIFO order
        while let Some(id) = self.env_stack.pop() {
            out.extend_from_slice(b"\\end{");
            out.extend_from_slice(ENV_NAMES[id as usize]);
            out.push(b'}');
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn heals_braces_delims_and_envs() {
        let mut st = State::new();
        let mut out = Vec::new();
        st.heal(b"\\begin{pmatrix}\\left(\\frac{1}{2", &mut out);
        assert_eq!(out, b"\\begin{pmatrix}\\left(\\frac{1}{2}\\right.\\end{pmatrix}");
    }
}
