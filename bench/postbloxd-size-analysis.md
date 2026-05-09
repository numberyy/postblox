# postbloxd Binary Size Analysis

Generated against `df14b8d` on 2026-05-09 (UTC).

Tool: `cargo bloat 0.12.1` (installed via `cargo install --locked`),
augmented with `cargo tree --duplicates` and `cargo tree -e features`.

This is an **investigation-only** report: the orchestrator decides whether
to act. No code or configuration files have been changed.

## Current state

Stripped sizes from `target/release/` after a clean release build at
`df14b8d`:

| Binary | Bytes | MiB | vs target (5 MB) | vs hard limit (10 MB) |
|---|---|---|---|---|
| `postbloxd` | 6 128 408 | 5.84 MiB (6.13 MB) | over (~1.13 MB drift) | well under |
| `postblox` (TUI) | 1 660 592 | 1.58 MiB | under | under |
| `postblox-mcp` | 1 162 224 | 1.11 MiB | under | under |
| `postblox-bench` | 1 009 232 | 0.96 MiB | under | under |

`file target/release/postbloxd` confirms it is `stripped`. `postbloxd` is
the largest binary by ~3.5×.

## Release profile

`Cargo.toml [profile.release]` matches the CLAUDE.md specification:

| Setting | CLAUDE.md spec | Cargo.toml | Status |
|---|---|---|---|
| `opt-level` | `"z"` | `"z"` | ✓ |
| `lto` | `true` | `true` | ✓ |
| `codegen-units` | `1` | `1` | ✓ |
| `strip` | `true` | `true` | ✓ |
| `panic` | `"abort"` | `"abort"` | ✓ |

**No profile flag is missing or weaker than spec.** The size drift is
not coming from a sub-optimal release profile.

## Top contributors by crate (postbloxd)

`cargo bloat --release --bin postbloxd --crates -n 60`. Reported `.text`
sizes (note: bloat cannot perfectly attribute every byte; LTO erases
crate boundaries). The numbers below are guidance, not contracts.

| Rank | Crate | Size | Notes |
|---|---|---|---|
| 1 | `[Unknown]` | 675.2 KiB | Mostly bundled SQLite C code (`sqlite3VdbeExec`, `sqlite3Select`, `sqlite3Parser`, `sqlite3Pragma`, `sqlite3WhereBegin`, `sqlite3Insert` …). |
| 2 | `postblox` | 527.3 KiB | First-party crate; dispatcher generic-monomorphization is the largest single function. |
| 3 | `std` | 515.6 KiB | Rust standard library + backtrace symbol tables. |
| 4 | `rustls` | 270.1 KiB | TLS implementation. Used for IMAP, SMTP, OAuth, embeddings HTTP. |
| 5 | `ring` | 177.4 KiB | Crypto provider for `rustls` / `lettre` / `tokio-rustls`. |
| 6 | `sqlx_sqlite` | 118.5 KiB | SQLx's SQLite glue layer. |
| 7 | `reqwest` | 107.1 KiB | HTTP client (used for OAuth + embeddings). Pulls hyper / hyper-util. |
| 8 | `dbus` | 97.2 KiB | Pulled in by `keyring`'s Linux fallback (`secret-service`). |
| 9 | `regex_automata` | 93.4 KiB | Used by `tracing-subscriber`'s env filter. |
| 10 | `regex_syntax` | 81.9 KiB | Same as above. |
| 11 | `imap_proto` | 65.8 KiB | IMAP protocol parser. |
| 12 | `tokio` | 65.2 KiB | Async runtime. |
| 13 | `mail_parser` | 56.3 KiB | MIME parser. |
| 14 | `hyper_util` | 46.8 KiB | Pulled in via `reqwest`. |
| 15 | `tracing_subscriber` | 45.2 KiB | env-filter feature is the heavy bit. |
| 16 | `webpki` | 40.1 KiB | Cert chain verification. |
| 17 | `sqlx_core` | 38.7 KiB | SQLx core abstractions. |
| 18 | `libsqlite3_sys` | 36.7 KiB | The Rust glue around the SQLite C code. |
| 19 | `lettre` | 34.2 KiB | SMTP submission. |
| 20 | `keyring` | 32.7 KiB | OS keyring abstraction. |
| 21 | `dbus_secret_service` | 28.9 KiB | Linux secret-service backend (transitive via `keyring`). |
| 22 | `serde_json` | 27.0 KiB | JSON for IPC frames + OAuth. |
| 23 | `http`, `url`, `encoding_rs`, `chrono`, `idna` | ~22 KiB each | Standard web/text plumbing. |

`.text` section total: 3.5 MiB (about 28% of the 12.5 MiB unstripped
binary). The remaining ~9 MiB pre-strip is symbols/debug/relocations,
mostly removed by `strip = true`.

## Top contributors by function (postbloxd)

`cargo bloat --release --bin postbloxd -n 50`. Top 20 shown:

| Size | Crate | Function |
|---|---|---|
| 157.9 KiB | postblox | `<DaemonDispatcher as Dispatcher>::dispatch::{{closure}}` |
| 36.0 KiB | postbloxd | `postbloxd::main::{{closure}}` |
| 32.6 KiB | rustls | `HandshakeMessagePayload::read_version` |
| 25.6 KiB | postblox | `sync::reconciler::reconcile_folder::{{closure}}` |
| 25.1 KiB | [SQLite C] | `sqlite3VdbeExec` |
| 23.3 KiB | postblox | `db::connect::{{closure}}` |
| 21.1 KiB | encoding_rs | `VariantDecoder::decode_to_utf8_raw` |
| 20.3 KiB | std | `core::ops::function::FnMut::call_mut` (catch-all) |
| 19.7 KiB | regex_automata | `dfa::dense::Builder::build_from_nfa` |
| 18.2 KiB | std | `backtrace_rs::symbolize::gimli::Cache::with_global` |
| 17.1 KiB | sqlx_sqlite | `connection::explain::explain` |
| 16.8 KiB | postblox | `oauth::google::GoogleOAuthHttpClient::post_token_form::{{closure}}` |
| 16.2 KiB | hyper_util | `Client<C,B>::send_request::{{closure}}` |
| 16.1 KiB | dbus | `arg::Iter::get_refarg` |
| 15.4 KiB | regex_automata | `nfa::thompson::compiler::Compiler::build_many` |
| 15.4 KiB | imap_proto | `nom::branch::Alt::choice` (15-tuple monomorphization) |
| 14.8 KiB | mail_parser | `parse_headers` |
| 14.6 KiB | postblox | `<toml::de::Deserializer as serde::Deserializer>::deserialize_struct` |
| 13.9 KiB | [SQLite C] | `sqlite3Select` |
| 13.0 KiB | [SQLite C] | `sqlite3Parser` |

The single largest function is `DaemonDispatcher::dispatch` (157.9 KiB).
Each IPC operation arm gets monomorphized inline; this is a function
worth profiling but not necessarily worth structurally rewriting.

## Crates only in postbloxd, not TUI (the daemon-only diff)

`postblox` (TUI) is 1.58 MiB. `postbloxd` is 5.84 MiB. The 4.26 MiB gap
maps to crates the TUI does not pull in:

| Crate | postbloxd | postblox TUI | Reason it's daemon-only |
|---|---|---|---|
| `[Unknown]` (SQLite C) | 675.2 KiB | not present | Daemon owns the DB; TUI talks to daemon over IPC. |
| `rustls` + `ring` + `webpki` + `rustls-pki-types` | 270 + 177 + 40 + 8 = ~495 KiB | not present | Daemon does outbound TLS for IMAP/SMTP/OAuth/embeddings; TUI does not. |
| `reqwest` + `hyper` + `hyper_util` + `hyper_rustls` | 107 + 15 + 47 + 6 = ~175 KiB | not present | OAuth + embeddings client. |
| `sqlx_sqlite` + `sqlx_core` + `libsqlite3_sys` | 118 + 39 + 37 = ~194 KiB | not present | Daemon-only DB layer. |
| `dbus` + `dbus_secret_service` | 97 + 29 = ~126 KiB | not present | Pulled by `keyring` Linux backend. |
| `regex_automata` + `regex_syntax` | 93 + 82 = ~175 KiB | not present | `tracing-subscriber`'s env filter — the daemon initialises tracing in `main`; TUI does not. |
| `imap_proto` + `async_imap` + `mail_parser` | 66 + 15 + 56 = ~137 KiB | not present | Sync worker. |
| `lettre` + `email-encoding` + `email_address` | ~34 KiB visible + ~9 KiB transitive = ~45 KiB | not present | SMTP submission. |
| `keyring` | 32.7 KiB | not present | Secret store. |
| `aes` + `aes-gcm` + `argon2` + `blake2` + `sha2` | ~32 KiB | not present | File secrets backend. |
| `idna` | 22 KiB | not present | URL/email host normalization (lettre/reqwest). |
| `encoding_rs` | 23 KiB | not present | Charset decoding for `mail_parser`. |
| `tracing_subscriber` | 45.2 KiB | not present | env-filter init. |

Total daemon-only `.text`: roughly **2.0 MiB** out of `postbloxd`'s
3.5 MiB `.text` section. After stripping, that 2.0 MiB roughly accounts
for the ~4 MiB delta on disk (the inflation factor matches the
strip-vs-rodata ratio observed across the four binaries).

## Feature flag audit

For each top-impact crate:

### `tokio` — features used

```toml
tokio = { version = "1", features = ["rt", "macros", "rt-multi-thread", "sync", "fs", "net", "io-util", "io-std", "signal", "time"] }
```

Already explicit; no `"full"`. **No safe trim** without breaking the
runtime: `rt-multi-thread` (work-stealing scheduler), `sync` (channels),
`fs` / `net` (DB + sockets), `signal` (graceful shutdown), `time`
(timeouts). Verdict: **already tight**.

### `reqwest` — features used

```toml
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
```

`default-features = false` is set. `json` is required for OAuth/embeddings
JSON bodies. `rustls-tls` is the TLS we want. Verdict: **already tight**.

### `rustls` / `tokio-rustls` — features used

```toml
tokio-rustls = { version = "0.26", default-features = false, features = ["tls12", "ring"] }
rustls = { version = "0.23", default-features = false, features = ["std", "tls12", "ring"] }
```

`default-features = false`, `tls12` only (no `tls13`?). **Inspection:**
the build still pulls TLS 1.3 code through `webpki` and the standard
TLS 1.3 ciphersuite set. Even with `tls12` listed alone, modern Gmail /
Office365 IMAP & SMTP servers will negotiate TLS 1.3, so dropping
TLS 1.3 is a non-starter. The current selection is **as small as
practical** without breaking real providers.

### `lettre` — features used

```toml
lettre = { version = "0.11", default-features = false, features = ["smtp-transport", "tokio1-rustls-tls", "builder"] }
```

`default-features = false` is set. The three features are all required:
`smtp-transport` is the actual submitter, `tokio1-rustls-tls` is the
TLS connector for tokio, `builder` is the `Message::builder` API used in
`postblox::mail::builder`. Verdict: **already tight**.

### `async-imap` — features used

```toml
async-imap = { version = "0.11", default-features = false, features = ["runtime-tokio"] }
```

`default-features = false`. Only `runtime-tokio` is enabled (vs the default
which pulls async-std too). Verdict: **already tight**.

### `keyring` — features used

```toml
keyring = { version = "3.6", features = ["apple-native", "linux-native-sync-persistent", "crypto-rust"] }
```

This is the heaviest "could be lighter" candidate.
- `linux-native-sync-persistent` pulls `dbus` (97.2 KiB) +
  `dbus_secret_service` (28.9 KiB) = ~126 KiB just for the Linux
  Secret Service backend.
- `crypto-rust` is the `aes-gcm` backend for the `crypto` feature
  (used to encrypt items going into Secret Service).
- `apple-native` pulls in macOS Keychain bindings — on Linux these are
  excluded by `cfg`, so the cost is near zero. (Cross-platform
  compilation does not light this up on Linux.)

There is no smaller default backend on Linux that still talks to GNOME
Keyring / KWallet. Removing `linux-native-sync-persistent` would mean
postblox loses OS keyring support on Linux entirely, falling back to
the AES-GCM file backend only.

### `mail-parser`, `chrono`, `serde`, `serde_json`, `sqlx`

All using sane feature selections. `sqlx` pulls only sqlite + tokio +
chrono + uuid + migrate + macros — no postgres/mysql.

### `tracing-subscriber` — features used

```toml
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

`env-filter` pulls `regex` → `regex_automata` (93.4 KiB) +
`regex_syntax` (81.9 KiB) ≈ **175 KiB**. This is the largest single
"avoidable" cost in the daemon-only diff.

If `RUST_LOG`-style filtering is acceptable to drop, switching to a
fixed level (e.g. `tracing_subscriber::fmt().with_max_level(...)`)
removes the regex dependency outright. But this would cost the ability
to scope log levels per-target, which is helpful for debugging IMAP
issues without spamming the dispatcher.

### `mime_guess`, `arboard`

Both already use `default-features = false`. `arboard` is TUI-only, not
in `postbloxd`. Good.

## Duplicate dependencies

`cargo tree --duplicates` finds the following crates compiled at multiple
versions. Each duplicate inflates the binary because both versions get
linked. Annotation column explains the source.

| Crate | Versions | Source of duplication |
|---|---|---|
| `async-channel` | 1.9.0, 2.5.0 | `stop-token` (transitive of `async-imap`) pins 1.9; `async-imap` itself uses 2.5. Upstream `async-imap` issue. |
| `chrono` | 0.4.44 (×2 instances in the graph) | Same version, but listed twice because of distinct feature unification roots. **Not a true duplicate** — same crate, same version. |
| `either` | 1.15.0 (×2) | Same — feature-graph artefact, single version. |
| `event-listener` | 2.5.3, 5.4.1 | `async-channel 1.9` uses `event-listener 2`; `sqlx-core` and `async-channel 2` use `event-listener 5`. |
| `futures-channel` | 0.3.32 (×2) | Single version, listed twice in feature graph. |
| `futures-sink` | 0.3.32 (×2) | Single version. |
| `getrandom` | 0.2.17, 0.4.2 | `rand_core 0.6` (used by `aes-gcm`/`argon2`/`uuid`) → 0.2; `rand_core 0.9` (used by `rustls 0.23`) → 0.4. Common in the rust-crypto ecosystem mid-migration. |
| `hashbrown` | 0.15.5, 0.17.0 | `dashmap 6` uses 0.15; `indexmap` already on 0.17. Mid-migration. |
| `linux-raw-sys` | 0.4.15, 0.12.1 | `rustix 0.38` (transitive via async tooling) uses 0.4; `rustix 1.x` (used directly by `arboard`'s `gethostname`) uses 0.12. |
| `log` | 0.4.29 (×2) | Single version, feature-graph artefact. |
| `mio` | 1.2.0 (×2) | Single version. |
| `nom` | 7.1.3, 8.0.0 | `imap-proto` (via `async-imap`) on 7; `lettre` on 8. **Real duplicate**, ~50–100 KiB cost. |
| `num-traits` | 0.2.19 (×2) | Single version. |
| `rustix` | 0.38.44, 1.1.4 | Same root cause as `linux-raw-sys`. **Real duplicate.** |
| `thiserror` / `thiserror-impl` | 1.0.69, 2.0.18 | `async-imap` still on 1; rest of the workspace on 2. **Real duplicate**, ~30–50 KiB. |
| `unicode-width` | 0.1.14, 0.2.0 | `unicode-truncate` (via `ratatui`) on 0.1; `ratatui 0.29` directly on 0.2. **Real duplicate**, but `ratatui` is TUI-only and barely shows up in `postbloxd`. |
| `uuid` | 1.23.1 (×2) | Single version. |

**Real (different-version) duplicates that hit `postbloxd` materially:**
`async-channel`, `event-listener`, `getrandom`, `hashbrown`, `linux-raw-sys`,
`nom`, `rustix`, `thiserror`/`thiserror-impl`. None of these are fixable
inside this repo without forking or pinning a transitive — they'll resolve
when upstreams (`async-imap`, `rustls`, `aes-gcm`, `dashmap`) update.
Combined cost is plausibly 150–250 KiB.

## Candidate optimizations (ordered by estimated savings)

1. **Drop `tracing-subscriber`'s `env-filter` feature** —
   est savings: **~150–175 KiB** —
   risk: **medium**.
   Switch to a fixed-level subscriber (`tracing_subscriber::fmt()` with
   `with_max_level` driven by a single env var or a config knob).
   Removes `regex_automata` + `regex_syntax`. Cost: lose per-target log
   filtering (e.g. `RUST_LOG=postblox::sync=trace,info`), which is
   genuinely useful for debugging the sync worker. Reversible.

2. **Disable `keyring`'s `linux-native-sync-persistent` and rely only on
   the AES-GCM file backend on Linux** —
   est savings: **~120–140 KiB** (drops `dbus` 97 KiB +
   `dbus_secret_service` 29 KiB) —
   risk: **high**.
   Removes Secret Service / GNOME Keyring / KWallet integration on
   Linux. Per CLAUDE.md the "OS keyring default" is the documented R5
   choice, and dropping it breaks that contract. Probably **not worth
   it**.

3. **Wait out the `nom 7 → 8` and `thiserror 1 → 2` migrations in
   transitive deps** —
   est savings: **~80–130 KiB** combined when `async-imap` updates —
   risk: **low** but **outside our control**.
   Tracks `async-imap`'s upstream. Worth a periodic re-check; nothing to
   do today.

4. **Wait out the `rustix 0.38 → 1.x` migration** —
   est savings: **~40–60 KiB** (linux-raw-sys + rustix dedup) —
   risk: **low**, **outside our control**.
   Same story as #3.

5. **Replace `reqwest` with a thinner client (e.g. `ureq` blocking, or
   `hyper` directly with `tokio-rustls`)** —
   est savings: **~150–200 KiB** (loses `hyper_util` + most of
   `reqwest`'s middleware machinery) —
   risk: **high**.
   `reqwest` is used in two well-isolated places: `oauth::google` and
   `embeddings`. Both could in principle be hand-rolled on `hyper` since
   we already have it transitively. Cost: more code to maintain, more
   tests to write, and we lose `reqwest`'s connection pooling / redirect
   handling that we may need later. **Net negative** unless size becomes
   a hard constraint.

6. **Audit `postblox::daemon::dispatcher::dispatch` (157.9 KiB
   monomorphized closure) for genericity that explodes** —
   est savings: **unknown, plausibly 20–60 KiB** —
   risk: **medium**.
   The dispatcher is one giant async match. Each arm captures a clone of
   the env. Splitting into per-op handler functions (with shared state
   passed by `Arc`) might reduce inlining pressure under LTO. Needs a
   focused experiment; could equally make things worse.

7. **Drop `chrono` in favour of `time` 0.3** —
   est savings: **~15–22 KiB** —
   risk: **medium**.
   `chrono` is used in `postblox::models` and `db` for timestamps.
   `time` has tighter codegen but breaking change in the API surface.
   Not worth the churn for ~20 KiB.

## Non-candidates considered and rejected

- **Tightening the release profile**: already at the CLAUDE.md spec
  (`opt-level = "z"`, `lto = true`, `codegen-units = 1`,
  `strip = true`, `panic = "abort"`). Nothing further to extract.
- **Switching to MUSL static linking**: would *grow* the binary (statically
  links `libc` etc.). Not relevant to the size goal.
- **`cargo-shrink` / `wasm-opt`-style tools**: not applicable to native
  Linux ELF.
- **Disabling `tokio` features further**: every enabled feature is
  load-bearing.
- **Disabling `sqlx` `macros` feature**: macros are used at compile-time
  only and don't appear in the final binary.
- **Splitting `postbloxd` into a thinner `postbloxd-core` + plugin
  binaries for IMAP/SMTP**: violates "The One Rule" and the dynamic
  linking complexity dwarfs the saving.
- **Dropping `lettre` and hand-writing SMTP**: `lettre` is already
  feature-trimmed; the size cost (34 KiB) is below any reasonable
  rewrite budget.
- **Replacing SQLite with a smaller embedded DB (e.g. `redb`,
  `sled`)**: changes the data model documented in CLAUDE.md
  (2026-05-06 decision: "SQLite, single-file"). Out of scope for size.
- **Replacing `rustls` + `ring` with native OpenSSL**: would grow the
  binary on most distros (still pulls `ring` transitively from
  `lettre` / `tokio-rustls` config) and conflict with the
  `rustls-platform-verifier` strategy.
- **Stripping further with `strip --strip-all`**: `strip = true` in the
  release profile is already `--strip-all` equivalent; `file` confirms
  the binary is `stripped`.
- **Dropping `regex` from `tracing-subscriber` by patching the crate**:
  upstream maintained, would be a fork. Equivalent to candidate #1
  but more invasive.

## Recommendation

**Leave `postbloxd` at 5.84 MiB.** It is **41% under the 10 MB hard
limit** with substantial headroom. The drift over the 5 MB target comes
overwhelmingly from features that are explicitly part of the project's
scope:

- TLS for IMAP/SMTP/OAuth/embeddings (~495 KiB),
- bundled SQLite (~675 KiB),
- the OS keyring on Linux (~126 KiB),
- env-filter logging (~175 KiB).

Each of these is justified by an existing requirement in CLAUDE.md or
the R5/R6 phase docs. The realistic safe ceiling for cuts (candidates
1, 3, 4 — the ones that don't break a documented behaviour) is roughly
**250–350 KiB**, which would bring `postbloxd` to ~5.5 MiB — still over
the 5 MB target.

Reaching the 5 MB target without giving up the OS keyring integration
(candidate 2, "high risk") or rewriting the HTTP layer (candidate 5,
"high risk") does not look feasible.

**Suggested follow-ups (no action required):**

1. Re-run this analysis when `async-imap` ships a release that drops
   `nom 7` and `thiserror 1` — that's a **free** ~80–130 KiB if it
   happens.
2. Periodically re-run `cargo tree --duplicates` after dependency bumps;
   the duplicate set shrinks naturally as the ecosystem migrates.
3. If size ever has to come under 5 MB (e.g. for a constrained
   distribution channel), the only realistic levers are candidates 1
   (env-filter) and 5 (drop `reqwest`). Both are reversible decisions
   to revisit later, not today.

The H1 baseline's `⚠` marker on this metric remains accurate: under the
hard limit, over the target, with a clear paper trail for why.
