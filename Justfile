# Justfile - Systems Engineering Command Orchestrator
# Architecture: Terminal Graphical Coprocessor for Math/Diagrams (ii)

set dotenv-load := false
set export := false
set shell := ["bash", "-euo", "pipefail", "-c"]

bin := "ii"
target_dir := "target"

default: qa

# ==============================================================================
# Domain 1: Cargo Development Lifecycle
# ==============================================================================

native := "-C target-cpu=native"

# Format code in-place according to rustfmt.toml
fmt:
    cargo fmt --all

# Strict read-only check for CI & Git hooks (fails on diff)
fmt-check:
    cargo fmt --all -- --check

# Strict lint gate: pedantic + nursery, warnings are errors (Cargo.toml [lints] + -D warnings)
lint *args:
    cargo clippy --locked --all-targets --all-features {{ args }} -- -D warnings

# Compact one-line-per-diagnostic lint output (file:line:col) for fast triage
lint-short:
    cargo clippy --locked --lib --bins --all-features --message-format=short -- -D warnings 2>&1 | grep -E "^(src|benches|tests)" || true

# Apply compiler and clippy machine-applicable fixes in a dirty tree, re-format, and show blast radius
fix:
    cargo fix --allow-dirty --allow-staged --all-targets --all-features
    cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features
    cargo fmt --all
    git diff --stat -- src benches tests

# Run complete verification suite (style check + pedantic/nursery lints)
qa: fmt-check lint

# Run all process-isolated tests via nextest with immediate failure output & doctests
test *args:
    cargo nextest run --locked --lib --bins --tests --all-features --status-level all --failure-output immediate-final {{ args }}
    cargo test --locked --doc --all-features

# Fast developer test pass (fail-fast, no doctests)
test-fast *args:
    cargo nextest run --locked --lib --bins --tests --all-features --fail-fast {{ args }}

# Run tests via standard cargo test harness with full backtraces
test-all *args:
    env RUST_BACKTRACE=1 cargo test --locked --lib --bins --tests --all-features {{ args }}

# Run IXPC zero-fork UDS daemon end-to-end tests (tests/daemon.rs)
e2e:
    cargo test --locked --test daemon --all-features

# Run the render pipeline micro-benchmarks with native host vectorization (NEON / AVX2)
bench *args:
    env RUSTFLAGS="{{ native }}" cargo bench --locked --bench bench {{ args }}

# Run the fork/exec vs zero-fork UDS daemon latency benchmark
bench-daemon *args:
    env RUSTFLAGS="{{ native }}" cargo bench --locked --bench daemon {{ args }}

# Run all benchmark targets with native vectorization
bench-all *args:
    env RUSTFLAGS="{{ native }}" cargo bench --locked {{ args }}

# Quick benchmark smoke pass (sample-size 10 for rapid regression detection)
bench-quick *args:
    env RUSTFLAGS="{{ native }}" cargo bench --locked --bench bench {{ args }} -- --sample-size 10

# Save Criterion baseline for A/B comparison (usage: just bench-save <name>)
bench-save name:
    env RUSTFLAGS="{{ native }}" cargo bench --locked --bench bench -- --save-baseline {{ name }}

# Compare against saved Criterion baseline (usage: just bench-compare <name>)
bench-compare name:
    env RUSTFLAGS="{{ native }}" cargo bench --locked --bench bench -- --baseline {{ name }}

# Profile pipeline throughput with macOS Instruments Time Profiler (writes .trace)
profile-instruments:
    env RUSTFLAGS="{{ native }}" cargo bench --locked --bench bench --no-run
    BIN=$(ls -t {{ target_dir }}/release/deps/bench-* | grep -v '\.d$' | grep -v '\.dSYM' | head -n1); \
    echo "Profiling $BIN via xctrace..."; \
    xcrun xctrace record --template 'Time Profiler' --launch -- "$BIN" --bench

# Run source-based LLVM coverage suite
cov:
    @mkdir -p {{ target_dir }}/llvm-cov
    cargo llvm-cov nextest --locked --all-features --profile ci --lcov --output-path {{ target_dir }}/llvm-cov/lcov.info

coverage: cov

# Continuous background compilation & testing
watch:
    bacon

# ==============================================================================
# Domain 2: Production Artifacts & Distribution
# ==============================================================================

# Build size-optimized release binary (opt-level = "z", strip = true)
release-small:
    cargo build --locked --profile release-small
    @ls -lh {{ target_dir }}/release-small/{{ bin }}

# Build full-throughput production release (opt-level = 3, LTO fat)
release:
    cargo build --locked --release
    @ls -lh {{ target_dir }}/release/{{ bin }}

# Host-tuned build kept separate from portable release artifacts.
release-native:
    env RUSTFLAGS="{{ native }}" cargo build --locked --release --target-dir {{ target_dir }}/native

# Run local cargo-dist release packaging dry run
dist-plan:
    dist plan

# ==============================================================================
# Domain 3: DevSecOps, Supply Chain & Workflows
# ==============================================================================

# Run supply chain security audit (RustSec, bans, licenses)
audit:
    cargo deny --all-features check

# Scan repository history and working tree for leaked secrets
audit-secrets:
    gitleaks detect --no-banner

# Scan for unused dependencies across all targets
machete:
    cargo machete

# Validate GitHub Actions workflows locally (actionlint + zizmor offline)
lint-workflows:
    actionlint -color
    zizmor --offline --min-severity high .github/workflows/

# Run memory safety & aliasing verification via Miri Tree Borrows
miri *args:
    env RUST_BACKTRACE=1 MIRIFLAGS="-Zmiri-disable-isolation -Zmiri-tree-borrows -Zmiri-strict-provenance" cargo +nightly miri test --bin ii {{ args }}

# Verify SemVer API backwards compatibility
semver:
    cargo semver-checks check-release

# Run mutation testing suite
mutants:
    cargo mutants

# Single-command composite gate (equivalent to local CI pass)
check-all: qa test audit audit-secrets machete lint-workflows

# ==============================================================================
# Domain 4: Tooling & Git Hooks
# ==============================================================================

# Install repository Git hooks via Lefthook
hooks-install:
    lefthook install

# Clean build artifacts
clean:
    cargo clean

# Install complete developer toolchain into workstation
setup-tools:
    brew install bacon sccache cargo-deny cargo-llvm-cov cargo-nextest cargo-dist cargo-mutants cargo-binstall lefthook cocogitto actionlint zizmor gitleaks
    cargo binstall -y cargo-machete cargo-semver-checks
    rustup toolchain install nightly --component miri,rust-src
