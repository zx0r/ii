# Contributing to ii

Thank you for considering contributing to **ii**!

## Guidelines & Principles

- **Zero Allocation in Hot Path**: Rendering buffers, scratch spaces, and arenas are preallocated in `Engine`. No heap allocations inside parsing and stream loops.
- **Data-Oriented Design (DOD)**: Prefer flat arrays (`Vec<Span>`, `Vec<RenderSlot>`) over pointers and nested hierarchies.
- **Sans-I/O Core**: Core parsing (`scan.rs`), protocol formatting (`kitty.rs`), and math compilation remain decoupled from I/O.
- **Zero Panic Policy**: `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]`, and `#![deny(clippy::panic)]` are strictly enforced.
- **Conventional Commits**: Commit messages must follow Conventional Commits 1.0.0 via `cog`.

## Quality Gates

Before opening a pull request, ensure all checks pass:

```bash
just check-all
```

This runs:
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo nextest run --all-targets`
- `cargo deny check`
- `gitleaks detect --no-banner`
- `cargo machete`
- `actionlint`
- `zizmor`

## Community & Questions

If you have questions or ideas, feel free to open an issue or discussion on GitHub.
