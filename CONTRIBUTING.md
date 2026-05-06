# Contributing to postblox

Thanks for your interest! postblox is small and pre-1.0 — clarity beats
ceremony. A few hard rules.

## Branch model

- **`main`** — the only branch on the public repo. Only advances when a
  release is tagged.
- Active development happens in a private mirror; the maintainer lands
  PRs there and ships them to `main` as part of a release.
- Open external PRs against `main`. Use topic branches:
  `feat/<name>`, `fix/<name>`, `docs/<name>`, `chore/<name>`.

## Before opening a PR

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check        # optional locally; CI enforces
```

CI runs the same checks across Linux + macOS plus an MSRV build. PRs
must be green before review.

## Commit style

[Conventional Commits](https://www.conventionalcommits.org/), imperative
mood, one logical change per commit.

```
feat: add inbox sync worker
fix: correct thread assignment for In-Reply-To headers
test: cover MIME boundary edge cases
docs: clarify MCP gate evaluation order
chore: bump sqlx to 0.8.6
```

Squash-merge. History stays linear.

## Code style

The project's hard rules live in [`CLAUDE.md`](CLAUDE.md). The short
version:

1. **Every PR must be usable by itself.** No trait setup for "the next PR."
2. **No abstractions before the third use.** Concrete first.
3. **Delete code that isn't tested.** Untested = liability.
4. **Bound everything.** No unbounded channels, queues, or pools.
5. **Comments explain WHY, not WHAT.** If you need a comment for the
   what, simplify the code instead.

## Tests

- New module → tests for that module. No exceptions.
- `mail/` and `embeddings/` MUST NOT import a framework crate.
- Integration tests against an in-memory pool: `db::test_pool()`.
- Edge cases (empty/zero, one, many, boundary, malformed, unicode,
  concurrent) are mandatory, not optional.

## Licensing your contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in postblox by you, as defined in the Apache-2.0
license, shall be dual-licensed as MIT OR Apache-2.0, without any
additional terms or conditions.

## Reporting bugs / asking questions

- **Bug reports**: open a GitHub issue with the bug template.
- **Security issues**: see [`SECURITY.md`](SECURITY.md). Don't open a
  public issue.
- **Feature requests**: open a discussion or issue with the feature
  template; small, surgical, justified.

## Code of Conduct

By participating you agree to abide by the
[Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).
