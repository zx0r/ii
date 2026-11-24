# Contributing to ii

Start with [README.md](README.md) for scope, formats, architecture, and constraints.

## Changing Behavior

1. Reproduce the issue with concrete input and conditions.
2. Locate the owner of the behavior; fix the shared path, not a one-host copy.
3. Add verification for content loss, wrong page, pixel corruption, or protocol violation.
4. Remove superseded logic and update the contract in documentation.
5. Run applicable checks and state what could not be verified.

## Verification

```sh
cargo +stable fmt --all -- --check
cargo +stable clippy --locked --all-targets --all-features -- -D warnings
cargo +stable test --locked
```

For render and adapter correctness, use real stands from `showcase/checks/`.
For performance, compare identical corpus and useful output.

The `Justfile` provides additional recipes requiring external cargo/tool dependencies.
Installing the full set is not required for documentation or local-only edits.
Presence of a recipe, Git hook, or CI workflow does not mean the check has been executed.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/) with precise description of the actual change.
Respect existing hooks and signing — do not bypass them.
Do not commit build artifacts, secrets, local logs, or user-specific configuration.
A commit is not authorization to push or publish a release.

In the commit body, state the result, reason, verification, and material constraints.
A new dependency requires technical justification and analysis of its transitive graph.
