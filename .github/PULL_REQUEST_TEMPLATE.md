## What

<!-- One paragraph: the change and why. Keep it short. -->

## Checklist

- [ ] PR targets `dev` (not `main`)
- [ ] One logical change; commit message follows conventional commits
- [ ] `cargo fmt --check` clean
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo test` green (locally and in CI)
- [ ] Tests added for new behaviour, including edge cases
- [ ] Public APIs documented; no untested code path landing
- [ ] `CHANGELOG.md` updated under `## [Unreleased]` if user-visible

## Related issues

<!-- Closes #123 -->
