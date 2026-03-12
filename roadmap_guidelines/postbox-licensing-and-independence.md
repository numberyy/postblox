# PostBox — Licensing, Ethics, and Dependency Independence Plan

## Addendum to the Technical Specification

---

## 1. License Audit — Every Dependency

### Linked Dependencies (compiled into the PostBox binary)

| Crate | License | Commercial use? | Risk |
|-------|---------|----------------|------|
| `tokio` | MIT | Unrestricted | None |
| `axum` | MIT | Unrestricted | None |
| `sqlx` | MIT OR Apache-2.0 | Unrestricted | None |
| `mail-parser` | MIT OR Apache-2.0 | Unrestricted | None |
| `mail-builder` | MIT OR Apache-2.0 | Unrestricted | None |
| `mail-send` | MIT OR Apache-2.0 | Unrestricted | None |
| `smtp-proto` | MIT OR Apache-2.0 | Unrestricted | None |
| `mail-auth` | MIT OR Apache-2.0 | Unrestricted | None |
| `async-imap` | MIT OR Apache-2.0 | Unrestricted | None |
| `lettre` | MIT | Unrestricted | None |
| `reqwest` | MIT OR Apache-2.0 | Unrestricted | None |
| `serde` | MIT OR Apache-2.0 | Unrestricted | None |
| `rustls` | MIT OR Apache-2.0 | Unrestricted | None |
| `oauth2` | MIT OR Apache-2.0 | Unrestricted | None |
| `argon2` | MIT OR Apache-2.0 | Unrestricted | None |

**Verdict: Every crate that PostBox links against is MIT or Apache-2.0. You can sell PostBox, close-source parts of it, modify it — no restrictions.**

### Runtime Dependencies (separate processes PostBox communicates with)

| Software | License | How PostBox uses it | Commercial implications |
|----------|---------|-------------------|----------------------|
| **Stalwart Mail Server** | **AGPL-3.0** (or commercial SELv1) | Separate process, HTTP/IMAP/SMTP API calls | See detailed analysis below |
| PostgreSQL | PostgreSQL License (permissive) | Separate process, SQL protocol | None whatsoever |
| Ollama (optional) | MIT | Separate process, HTTP API | None |

### The Stalwart AGPL Question — Detailed

**What AGPL-3.0 requires:**
- If you **modify** Stalwart and offer it as a network service → publish your modifications under AGPL
- If you **distribute** Stalwart (ship it in a Docker image, installer, etc.) → include source code and AGPL license notice
- If you **use unmodified Stalwart as a separate service** that your software talks to → **no obligations on your software**

**What PostBox does:**
PostBox talks to Stalwart over standard network protocols (HTTP REST API, IMAP, SMTP). PostBox does NOT:
- Link against Stalwart's server code
- Import Stalwart's AGPL-licensed modules
- Modify Stalwart's source
- Form a single combined work with Stalwart

This is the same relationship as "a web app talking to MySQL" — the app is not a derivative work of MySQL.

**Commercial scenarios:**

| Scenario | AGPL issue? | Action needed |
|----------|-------------|---------------|
| Sell PostBox as MIT open-source, users install Stalwart themselves | **No** | Recommend Stalwart in docs, but it's user's choice |
| Sell PostBox Pro (closed-source premium features) | **No** | PostBox doesn't contain any AGPL code |
| Run PostBox as hosted SaaS, Stalwart runs on your servers | **Probably no** | You're running unmodified AGPL software server-side. AGPL only triggers if you modify it. But: add legal disclaimer, consider Stalwart SELv1 license for peace of mind. |
| Ship Docker image with Stalwart bundled inside | **Yes, be careful** | You're distributing AGPL software. Must comply with AGPL for the Stalwart portion. Or: make Stalwart a user-installed prerequisite. |
| Fork/modify Stalwart for PostBox | **Yes** | Must publish modifications under AGPL. Or: buy SELv1. |

**Recommendation:** In the Docker Compose file, Stalwart is a separate service (separate image) that the user pulls from Stalwart's registry. PostBox never bundles Stalwart. This cleanly avoids all AGPL distribution obligations.

---

## 2. Ethical Approach to Open Source Dependencies

### 2.1 Core principles

**Give back more than you take.**
PostBox benefits enormously from Stalwart's crate ecosystem, async-imap, and the broader Rust async ecosystem. The ethical obligation isn't just legal compliance — it's active contribution.

**Be transparent about what's yours and what isn't.**
PostBox's README and docs should clearly credit every major dependency, especially Stalwart's crates. Don't create the impression that PostBox built email parsing from scratch.

**Don't compete unfairly with your dependencies.**
PostBox sits *on top of* Stalwart, not *instead of* it. Don't replicate Stalwart's server to avoid AGPL — that's both unethical and wasteful. If you eventually replace Stalwart, do it because your needs genuinely diverged, not to dodge a license.

### 2.2 Concrete actions

**Financial:**
- Sponsor Stalwart Labs (even $50/month signals good faith)
- Sponsor async-imap and other key crates
- If PostBox generates revenue, allocate 5% of net to upstream dependencies
- If PostBox raises money, make a one-time grant to Stalwart and other key deps

**Code contributions:**
- Any bugs found in mail-parser/mail-builder → upstream PR first, workaround second
- Any improvements to async-imap → upstream PR
- Performance benchmarks that benefit the ecosystem → publish openly
- Test fixtures (sample .eml files) → contribute to mail-parser's test suite

**Community:**
- Credit prominently in README: "PostBox is built on the shoulders of Stalwart's excellent crate ecosystem"
- Link to Stalwart, async-imap, and other key projects from PostBox's docs
- Don't bad-mouth dependencies publicly, even if they cause friction
- If PostBox users report Stalwart issues, help triage before filing upstream

**License reciprocity:**
- PostBox core: MIT (maximum adoption)
- If PostBox develops improvements that could benefit the email ecosystem generally (e.g., a better reply extraction algorithm, email threading library), publish these as separate MIT-licensed crates — don't hoard them inside PostBox

### 2.3 Stalwart relationship specifically

Stalwart Labs is a small company. PostBox bringing traffic/users to Stalwart is genuinely valuable to them. But PostBox also creates a risk — if PostBox becomes the "easy way" to use Stalwart, Stalwart Labs might feel their enterprise value is being commoditized.

**Mitigation:**
- Proactive outreach to Stalwart Labs before PostBox launches. "Hey, we're building X on top of Stalwart, here's how we think it benefits you, how can we work together?"
- Recommend Stalwart Enterprise in PostBox docs for users who need enterprise features (multi-tenancy, AI spam filtering, OIDC)
- If PostBox has a paid tier, include a "Stalwart Enterprise compatible" badge and potentially bundle SELv1 licenses for paying customers
- Never fork Stalwart. If you need server-level changes, propose them upstream.

---

## 3. Long-Term Dependency Replacement Roadmap

The goal isn't to replace everything — that's NIH syndrome. The goal is to have a credible path to independence for components where upstream risk is highest.

### 3.1 Risk assessment

| Dependency | Upstream risk | Replace? | Timeline |
|------------|--------------|----------|----------|
| **Stalwart server** | Medium — AGPL, small company, could change license or stall | **Yes, eventually** | Phase 7+ (year 2+) |
| **mail-parser** | Low — MIT, stable, no deps, could fork trivially | **No** | Never (unless abandoned) |
| **mail-builder** | Low — MIT, stable, minimal deps | **No** | Never (unless abandoned) |
| **mail-send** | Low — MIT, thin SMTP client layer | **Maybe** | If we need custom SMTP behavior |
| **async-imap** | Medium — small maintainer pool, crate marked "looking for maintainers" | **Yes** | Phase 5-6 |
| **tokio** | Near-zero — industry standard | **No** | Never |
| **axum** | Near-zero — Tokio team maintains it | **No** | Never |
| **sqlx** | Low — popular, Launchbadge maintains it | **No** | Never |
| **pgvector** | Low — PostgreSQL extension, widely used | **No** | Never |

### 3.2 Stalwart server replacement — "PostBox MTA"

This is the biggest and most important replacement. The goal is NOT to build a full mail server. It's to build the minimum viable MTA that handles PostBox's specific needs.

**What PostBox actually needs from Stalwart:**
1. Receive inbound SMTP and deliver to PostBox (not to a mailbox)
2. Send outbound SMTP with DKIM signing
3. SPF/DKIM/DMARC verification on inbound
4. TLS (DANE, MTA-STS)
5. Queue management for outbound with retries

**What PostBox does NOT need from Stalwart:**
- IMAP server (PostBox has its own API)
- JMAP server (PostBox has its own API)
- POP3 server
- CalDAV / CardDAV / WebDAV
- Web admin UI
- Sieve scripting
- Full mailbox storage

This dramatically reduces scope. PostBox-MTA would be:

```
Inbound:  SMTP listener → SPF/DKIM/DMARC check → deliver to PostBox via internal API
Outbound: PostBox internal API → DKIM sign → SMTP delivery with retry queue
```

**Implementation path:**
- `smtp-proto` (MIT) gives us the SMTP protocol parser
- `mail-auth` (MIT) gives us SPF/DKIM/DMARC verification and DKIM signing
- `mail-send` (MIT) gives us outbound SMTP delivery
- `rustls` (MIT) gives us TLS
- We build the glue: listener, queue, retry logic

**Estimated effort:** 3-4 months of focused work. This is a Phase 7+ project, after PostBox is established and the need for independence is real.

**When to trigger this:**
- Stalwart changes license terms unfavorably
- Stalwart stalls development and accumulates unpatched vulnerabilities
- PostBox's needs diverge from Stalwart's design (e.g., PostBox needs deep hooks into the SMTP pipeline that Stalwart won't expose)
- Commercial customers demand no AGPL anywhere in their stack, even as a runtime dependency

**Until then:** Stalwart is the right choice. It's mature, fast, and actively maintained. Building PostBox-MTA prematurely would be months of engineering better spent on PostBox's actual differentiators.

### 3.3 async-imap replacement — "PostBox IMAP Client"

The async-imap crate works but has risks:
- The original rust-imap is "looking for maintainers"
- async-imap is a community fork (chatmail/async-imap)
- IMAP is a complex protocol with many extensions

**Implementation path:**
- Build `postbox-imap` as a focused IMAP client that only implements what PostBox needs:
  - LOGIN/AUTHENTICATE (with OAuth2 SASL for Gmail)
  - SELECT
  - FETCH (headers, body, structure)
  - SEARCH
  - STORE (flags)
  - COPY/MOVE
  - IDLE (RFC 2177)
  - UID commands
- Use `imap-proto` (MIT) for protocol parsing — this handles the wire format
- Build the async session management ourselves with Tokio

**Estimated effort:** 6-8 weeks. This is tractable because we don't need a general-purpose IMAP library — just a focused client for sync.

**When to trigger:** Phase 5-6, when linked inbox sync is battle-tested and we know exactly which IMAP edge cases matter.

### 3.4 Dependency replacement priority order

```
Priority 1 (Phase 5-6): async-imap → postbox-imap
  Reason: Highest upstream risk, focused scope, manageable effort

Priority 2 (Phase 7+): Stalwart server → postbox-mta
  Reason: Biggest strategic risk, but also biggest effort.
  Only if triggered by license/maintenance/divergence concerns.

Priority 3 (never, unless forced): mail-parser, mail-builder
  Reason: MIT licensed, no deps, stable API, would take 6+ months to replicate.
  Fork-and-maintain is always available as a last resort.

Priority 4 (never): tokio, axum, sqlx, rustls, serde
  Reason: Industry infrastructure. Replacing these is like writing your own OS kernel.
```

---

## 4. Monetization Models — Ranked by Fit

### Model A: Open-core (recommended)

```
PostBox Community (MIT, free forever):
  - Full API (inboxes, messages, threads, search)
  - Native + linked inboxes
  - Permission engine
  - Webhooks + WebSockets
  - MCP server
  - Web dashboard
  - Single-org

PostBox Pro (commercial license, $X/month or one-time):
  - Multi-org / multi-tenant
  - Advanced analytics and deliverability tracking
  - Priority queue and batch operations
  - SSO / OIDC integration
  - Premium support
  - Audit log export and compliance reporting
  - Auto-scaling sync engine for 100+ linked accounts
```

**Why this works:** Community edition is genuinely useful on its own — not crippled. Pro features are things that only matter at scale or in enterprise contexts. The MIT license on the community edition means maximum adoption, which drives Pro conversions.

**License considerations:** PostBox binary contains only MIT/Apache code. No AGPL in the binary. Users install Stalwart separately. Zero license friction.

### Model B: Managed hosting

```
PostBox Cloud:
  - We run PostBox + Stalwart + Postgres for you
  - Per-inbox or per-message pricing
  - Compete directly with AgentMail but self-hosted option always available
```

**License considerations:** Running unmodified Stalwart on your servers is fine under AGPL. If you want extra comfort or enterprise features, buy a Stalwart SELv1 license ($X/year). Budget for this from revenue.

### Model C: Consulting + support

```
- Deployment consulting for large organizations
- Custom integration development
- Priority support contracts
- Training for teams adopting PostBox
```

**License considerations:** None. You're selling your time and expertise.

### Model D: Marketplace / ecosystem

```
- PostBox Plugin Marketplace (paid plugins for specific integrations)
- Certified partner program for consulting firms
- "Powered by PostBox" certification for products built on PostBox
```

**License considerations:** Plugins can be any license. PostBox core stays MIT.

### Recommended combination: A + B + C

Start with open-core (A) for product revenue. Add managed hosting (B) once you have enough users to justify the infra. Consulting (C) generates revenue from day one while you build.

---

## 5. PostBox's Own License Strategy

### The code you write

**PostBox core: MIT**

Rationale:
- Maximum adoption — no license friction for anyone
- Allows commercial use by others (this grows the ecosystem)
- Contributors don't worry about CLA implications
- Compatible with everything

**PostBox Pro extensions: Commercial / source-available**

If you go open-core, the Pro-only code is either:
- Fully proprietary (closed source)
- Source-available with a commercial license (BSL 1.1, SSPL, or custom)

BSL 1.1 (like MariaDB, CockroachDB, Sentry) is a good middle ground — the code is readable, converts to open source after a time delay (typically 3-4 years), and prevents cloud providers from offering it as a service without paying.

### Contributor License Agreement

For the MIT-licensed core, a CLA is optional but recommended if you plan to sell Pro features. The CLA should be lightweight:
- Contributors retain copyright
- They grant you a broad license to use their contributions (including in commercial products)
- This lets you include community contributions in Pro features without legal ambiguity
- Use a standard CLA like the Apache ICLA or HashiCorp CLA

### What NOT to do

- Don't start with AGPL for PostBox "just in case." It scares away contributors and commercial adopters.
- Don't relicense later from MIT to something restrictive (this burns community trust — see HashiCorp/Terraform controversy).
- Don't require copyright assignment in the CLA (this is aggressive and unnecessary).
- Don't use SSPL (MongoDB's license) — it's intentionally designed to be incompatible with everything and is widely disliked.

---

## 6. Updated Dependency Decision Matrix

Combining everything — license safety, replacement feasibility, and commercial risk:

```
┌───────────────────────────────────────────────────────────────────┐
│                   KEEP FOREVER (no replacement needed)            │
│                                                                   │
│  tokio (MIT) · axum (MIT) · sqlx (MIT) · serde (MIT)            │
│  rustls (MIT) · PostgreSQL (permissive) · pgvector (PostgreSQL)  │
│                                                                   │
│  These are foundational infrastructure. Replacing them would be   │
│  rewriting the universe. They're also the lowest-risk deps.      │
└───────────────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────────────────┐
│               KEEP + FORK IF ABANDONED (low risk)                │
│                                                                   │
│  mail-parser (MIT) · mail-builder (MIT) · mail-send (MIT)        │
│  mail-auth (MIT) · smtp-proto (MIT)                              │
│                                                                   │
│  MIT licensed, minimal/zero deps, stable APIs. If Stalwart Labs  │
│  abandoned these crates tomorrow, you fork and maintain. The     │
│  codebase is small enough (12K SLoC for mail-parser) that a      │
│  single developer can maintain a fork.                            │
└───────────────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────────────────┐
│            REPLACE IN PHASE 5-6 (medium risk)                    │
│                                                                   │
│  async-imap (MIT) → postbox-imap                                 │
│                                                                   │
│  Small maintainer pool. We use a focused subset of IMAP. Build   │
│  our own client using imap-proto for parsing. ~6-8 weeks effort. │
└───────────────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────────────────┐
│         REPLACE IF TRIGGERED (strategic risk)                    │
│                                                                   │
│  Stalwart server (AGPL) → postbox-mta                            │
│                                                                   │
│  AGPL is fine for current architecture (separate process, API    │
│  calls only). Build postbox-mta ONLY if:                         │
│    - Stalwart changes license unfavorably                        │
│    - Stalwart stalls and accumulates vulnerabilities             │
│    - PostBox needs deep SMTP pipeline hooks Stalwart won't add   │
│    - Enterprise customers require zero-AGPL stacks               │
│                                                                   │
│  All building blocks are MIT (smtp-proto, mail-auth, mail-send). │
│  Estimated effort: 3-4 months. Don't do this prematurely.        │
└───────────────────────────────────────────────────────────────────┘
```

---

## 7. Summary — The Honest Answer

**Can you monetize PostBox with Stalwart as a dependency?**
Yes. Unambiguously yes for open-core and consulting models. Yes with minor precautions for managed hosting. The only scenario that requires care is bundling Stalwart in a distributed package.

**Is anything blocking you?**
No. Every library PostBox links against is MIT or Apache-2.0. Stalwart the server is a separate runtime process, and communicating with it over standard protocols creates no license obligations on PostBox.

**What's the ethical path?**
Sponsor upstream. Contribute bug fixes. Credit prominently. Don't fork Stalwart to dodge AGPL — use it as intended and replace it only if your needs genuinely diverge. If PostBox makes money, share some with the projects that made it possible.

**What's the long-term independence path?**
The MIT-licensed Stalwart crates (mail-parser, mail-builder, mail-auth, smtp-proto) are your building blocks. When the time comes, you can build postbox-mta using these same crates without rewriting the hard email stuff. But don't do it until you need to.
