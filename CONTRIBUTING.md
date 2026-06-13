# Contributing to postblox

Thanks for your interest! postblox is small and pre-1.0 — clarity beats
ceremony. A few hard rules.

## Branch model

- **`main`** — stable; PRs land here.
- **`dev`** — active development.
- Open PRs against `main`. Use topic branches: `feat/<name>`,
  `fix/<name>`, `docs/<name>`, `chore/<name>`.

## Before opening a PR

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check        # optional locally; CI enforces
```

Or just `just check`. CI runs the same across Linux + macOS plus an MSRV
build. PRs must be green before review.

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

## Code style

These are hard rules, not suggestions — a violation is a PR comment.

1. **Every PR must be usable by itself.** No trait setup for "the next
   PR," no placeholder files, no empty modules with `todo!()`.
2. **No abstractions before the third use.** Write concrete first; extract
   a trait only when three real implementations create duplication.
   (Grouping arguments into a record/options struct is data, not an
   abstraction — that's fine.)
3. **Delete code that isn't tested.** Untested, unreachable code is a
   liability. No test and no live path → remove it.
4. **Bound everything.** No unbounded channels, queues, or pools. Every
   buffer has a cap.
5. **Comments explain WHY, not WHAT.** If you need a comment to say what
   the code does, simplify the code instead.
6. **`thiserror` per module — no giant `AppError`.** `anyhow` is confined
   to `src/bin/*` and `src/main.rs`. Error messages are lowercase with no
   trailing punctuation.
7. **Never `let _ = <Result>`** without a `// best-effort …` comment
   explaining why the error is dropped.
8. **No mutable global state** — no `lazy_static!`, no mutable statics.
   Immutable `OnceLock` handle caches (e.g. a shared HTTP client) are fine.
9. **Async is for real I/O only** (HTTP, DB, sockets, subprocesses).
   Parsing, formatting, theme resolution, and small state transitions stay
   sync.

### Layering

- `crates/postblox-mail/` MUST NOT import any framework crate — no tokio,
  sqlx, reqwest, ratatui, lettre, or anyhow. It owns the parser, builder,
  reply extraction, and threading as framework-free code.
- No module opens its own DB connection — the pool comes from the daemon.

### Naming

- `PascalCase` types/traits, `snake_case` fns/modules,
  `SCREAMING_SNAKE_CASE` consts. Acronyms as words: `Uuid`, `Mcp`, `Http`.
- 4-space indent, `max_width = 100`, edition 2021. Run `cargo fmt` on save.

## Tests

- New module → tests for that module. No exceptions.
- Integration tests run against an in-memory pool: `db::test_pool()`.
- Edge cases (empty/zero, one, many, boundary, malformed, unicode,
  concurrent) are mandatory, not optional.
- Test naming: `test_<thing>_<scenario>_<expected_result>`.
- Mock at trait boundaries, not at depth. Tag external/slow tests with
  `#[ignore]` plus a one-line reason.

## Licensing your contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in postblox by you, as defined in the Apache-2.0
license, shall be dual-licensed as MIT OR Apache-2.0, without any
additional terms or conditions.

## Reporting bugs / asking questions

- **Bug reports**: open a GitHub issue with the bug template.
- **Security issues**: see [`SECURITY.md`](SECURITY.md). Don't open a
  public issue.
- **Feature requests**: open an issue with the feature template; small,
  surgical, justified.

Be respectful and assume good faith. That's the whole code of conduct.
