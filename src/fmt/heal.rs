//! Bounded repair for incomplete live `LaTeX` input.
//!
//! Complete input is returned by reference. Repairs are appended only to caller-owned scratch.

use memchr::memchr;

const MAX_DEPTH: usize = 32;
const MAX_ENVS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Env {
    Aligned,
    Align,
    AlignStar,
    Gather,
    GatherStar,
    Gathered,
    Flalign,
    FlalignStar,
    Multline,
    MultlineStar,
    Matrix,
    PMatrix,
    BMatrix,
    BbMatrix,
    VMatrix,
    VvMatrix,
    SmallMatrix,
    Cases,
    DCases,
    RCases,
    DRCases,
    Split,
    Array,
    Subarray,
    Equation,
    EquationStar,
    AlignedAt,
}

impl Env {
    const fn name(self) -> &'static [u8] {
        match self {
            Self::Aligned => b"aligned",
            Self::Align => b"align",
            Self::AlignStar => b"align*",
            Self::Gather => b"gather",
            Self::GatherStar => b"gather*",
            Self::Gathered => b"gathered",
            Self::Flalign => b"flalign",
            Self::FlalignStar => b"flalign*",
            Self::Multline => b"multline",
            Self::MultlineStar => b"multline*",
            Self::Matrix => b"matrix",
            Self::PMatrix => b"pmatrix",
            Self::BMatrix => b"bmatrix",
            Self::BbMatrix => b"Bmatrix",
            Self::VMatrix => b"vmatrix",
            Self::VvMatrix => b"Vmatrix",
            Self::SmallMatrix => b"smallmatrix",
            Self::Cases => b"cases",
            Self::DCases => b"dcases",
            Self::RCases => b"rcases",
            Self::DRCases => b"drcases",
            Self::Split => b"split",
            Self::Array => b"array",
            Self::Subarray => b"subarray",
            Self::Equation => b"equation",
            Self::EquationStar => b"equation*",
            Self::AlignedAt => b"alignedat",
        }
    }

    const fn parse(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"aligned" => Self::Aligned,
            b"align" => Self::Align,
            b"align*" => Self::AlignStar,
            b"gather" => Self::Gather,
            b"gather*" => Self::GatherStar,
            b"gathered" => Self::Gathered,
            b"flalign" => Self::Flalign,
            b"flalign*" => Self::FlalignStar,
            b"multline" => Self::Multline,
            b"multline*" => Self::MultlineStar,
            b"matrix" => Self::Matrix,
            b"pmatrix" => Self::PMatrix,
            b"bmatrix" => Self::BMatrix,
            b"Bmatrix" => Self::BbMatrix,
            b"vmatrix" => Self::VMatrix,
            b"Vmatrix" => Self::VvMatrix,
            b"smallmatrix" => Self::SmallMatrix,
            b"cases" => Self::Cases,
            b"dcases" => Self::DCases,
            b"rcases" => Self::RCases,
            b"drcases" => Self::DRCases,
            b"split" => Self::Split,
            b"array" => Self::Array,
            b"subarray" => Self::Subarray,
            b"equation" => Self::Equation,
            b"equation*" => Self::EquationStar,
            b"alignedat" => Self::AlignedAt,
            _ => return None,
        })
    }

    const fn post_groups(self) -> u8 {
        match self {
            Self::Array | Self::Subarray | Self::AlignedAt => 1,
            _ => 0,
        }
    }
}

const fn arity(cmd: &[u8]) -> u8 {
    match cmd {
        b"frac" | b"dfrac" | b"tfrac" | b"binom" | b"dbinom" | b"tbinom" => 2,
        b"sqrt" | b"text" | b"mathrm" | b"mathbf" | b"mathit" | b"mathbb" | b"mathcal"
        | b"mathfrak" | b"operatorname" | b"overline" | b"underline" | b"vec" | b"hat"
        | b"widehat" | b"tilde" | b"widetilde" | b"bar" => 1,
        _ => 0,
    }
}

fn command_end(src: &[u8], start: usize) -> usize {
    let mut pos = start;
    while pos < src.len() && src[pos].is_ascii_alphabetic() {
        pos += 1;
    }
    pos.max((start + 1).min(src.len()))
}

#[inline]
fn update_env(
    cmd: &[u8],
    env: Env,
    envs: &mut [Env; MAX_ENVS],
    env_len: &mut usize,
    pending: &mut [u8; MAX_DEPTH],
    slot: usize,
) {
    if cmd == b"begin" {
        if *env_len < MAX_ENVS {
            envs[*env_len] = env;
            *env_len += 1;
            pending[slot] = pending[slot].saturating_add(env.post_groups());
        }
    } else if *env_len > 0 && envs[*env_len - 1] == env {
        *env_len -= 1;
    }
}

/// Repair an unfinished math fragment without allocating after the caller warms `scratch`.
#[must_use]
pub fn heal<'a>(raw: &'a [u8], scratch: &'a mut Vec<u8>) -> &'a [u8] {
    let mut braces = 0usize;
    let mut left = [0u8; MAX_DEPTH];
    let mut pending = [0u8; MAX_DEPTH];
    let mut envs = [Env::Aligned; MAX_ENVS];
    let mut env_len = 0usize;
    let mut pos = 0usize;
    let mut changed = false;

    while pos < raw.len() {
        match raw[pos] {
            b'%' => {
                pos = memchr(b'\n', &raw[pos..]).map_or(raw.len(), |n| pos + n + 1);
            }
            b'\\' => {
                let start = pos + 1;
                let end = command_end(raw, start);
                let cmd = &raw[start..end];
                if cmd == b"begin" || cmd == b"end" {
                    let mut at = end;
                    while at < raw.len() && raw[at].is_ascii_whitespace() {
                        at += 1;
                    }
                    let close = raw
                        .get(at..)
                        .filter(|tail| tail.first() == Some(&b'{'))
                        .and_then(|tail| memchr(b'}', &tail[1..]));
                    if let Some(close) = close {
                        let name = &raw[at + 1..at + 1 + close];
                        if let Some(env) = Env::parse(name) {
                            update_env(
                                cmd,
                                env,
                                &mut envs,
                                &mut env_len,
                                &mut pending,
                                braces.min(MAX_DEPTH - 1),
                            );
                        }
                        pos = at + close + 2;
                        continue;
                    }
                } else if cmd == b"left" {
                    left[braces.min(MAX_DEPTH - 1)] =
                        left[braces.min(MAX_DEPTH - 1)].saturating_add(1);
                } else if cmd == b"right" {
                    let slot = braces.min(MAX_DEPTH - 1);
                    left[slot] = left[slot].saturating_sub(1);
                } else {
                    pending[braces.min(MAX_DEPTH - 1)] =
                        pending[braces.min(MAX_DEPTH - 1)].saturating_add(arity(cmd));
                }
                pos = end;
            }
            b'{' => {
                let slot = braces.min(MAX_DEPTH - 1);
                pending[slot] = pending[slot].saturating_sub(1);
                braces = braces.saturating_add(1);
                pos += 1;
            }
            b'}' => {
                braces = braces.saturating_sub(1);
                pos += 1;
            }
            _ => {
                pos += 1;
            }
        }
    }

    let needs =
        braces > 0 || env_len > 0 || left.iter().any(|n| *n > 0) || pending.iter().any(|n| *n > 0);
    if !needs {
        return raw;
    }
    scratch.clear();
    scratch.extend_from_slice(raw);
    for depth in (0..=braces.min(MAX_DEPTH - 1)).rev() {
        while pending[depth] > 0 {
            scratch.extend_from_slice(b"{}");
            pending[depth] -= 1;
            changed = true;
        }
        while left[depth] > 0 {
            scratch.extend_from_slice(b"\\right.");
            left[depth] -= 1;
            changed = true;
        }
        if depth > 0 {
            scratch.push(b'}');
            changed = true;
        }
    }
    while env_len > 0 {
        env_len -= 1;
        scratch.extend_from_slice(b"\\end{");
        scratch.extend_from_slice(envs[env_len].name());
        scratch.push(b'}');
        changed = true;
    }
    if changed { scratch.as_slice() } else { raw }
}

#[cfg(test)]
mod tests {
    use super::heal;

    #[test]
    fn closes_bounded_constructs() {
        let mut out = Vec::new();
        assert_eq!(heal(br"\frac{a", &mut out), br"\frac{a}{}");
        assert_eq!(
            heal(br"\begin{matrix}a", &mut out),
            br"\begin{matrix}a\end{matrix}"
        );
        assert_eq!(heal(br"\left( x", &mut out), br"\left( x\right.");
    }
}
