# Security policy

postblox is a local-first email client and an MCP bridge that handles
real credentials and real mail. Security issues are taken seriously.

## Reporting a vulnerability

**Do not open a public issue.** Email the maintainer privately or use
GitHub's [private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability).

Please include:

- A clear description of the issue and its impact.
- Steps to reproduce, or a minimal proof-of-concept.
- The version (commit SHA) you tested against.
- Any suggested mitigation.

You will get an acknowledgement within 7 days. Critical issues will be
patched on `dev` and released as a point version on `main` as soon as a
fix is verified.

## Supported versions

postblox is pre-1.0. Only the latest release on `main` is supported for
security updates.

## Scope

In-scope:

- Credential leakage (IMAP/SMTP passwords, OAuth tokens, keyring entries)
- Path-traversal or injection in IPC ops, MCP tools, or DB queries
- MCP gate-engine bypasses (a tool call should not be allowed past a gate
  it shouldn't have matched)
- Local privilege escalation through the Unix socket
- Denial of service via crafted mail or RPC payloads

Out of scope:

- Bugs in upstream IMAP/SMTP servers
- Bugs in dependencies that have not yet been published with a fix
- Issues that require a compromised local user account to begin with
