# Security Policy

Memory Vault is a personal memory vault for AI agents. It holds what people
choose to tell their assistants about themselves — their job, their health,
their family. We take reports about it seriously.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report it privately through GitHub:

> **[Open a private security advisory](https://github.com/shahbaz242630/zaaheen/security/advisories/new)**

That channel is private to the maintainers until an advisory is published, and
it lets us work through the fix with you before anything becomes public.

If you can, include: what you found, how to reproduce it, which version or
commit you were on, and what you think the impact is. A proof of concept helps
enormously but is not required — a clear description of the flaw is welcome on
its own.

## What to expect

This is a **two-person project**, not a company with a security team. We will
not promise a response time we cannot keep. What we will do:

- Acknowledge your report as soon as we have actually read and understood it.
- Tell you honestly whether we agree it is a vulnerability, and why.
- Keep you updated while we work on it, rather than going quiet.
- Credit you when the fix ships, unless you would rather stay anonymous.

If you do not hear back within two weeks, please assume it was missed rather
than ignored, and nudge the advisory thread.

## What we consider a vulnerability

The guarantee this project is built around is:

> **The server cryptographically cannot read user vault contents.**

Anything that breaks that, or that exposes a user's memories to someone who
should not have them, is in scope. Concretely, we especially want to hear about:

- **Data readable at rest.** Any vault artifact written to disk unencrypted, or
  any path that bypasses the sealing envelope. We have shipped this bug before
  (see `ADR-SEC-007` in `HANDOFF.md`) and take repeats of the class seriously.
- **Boundary escapes.** A read or search returning memories from a boundary the
  caller was not authorized for.
- **Key handling.** Key material leaking into logs, crash dumps, temp files,
  swap, or process memory longer than necessary.
- **Memory or context poisoning.** Content stored in a vault that manipulates
  the behaviour of an AI agent reading it — for example, text that an agent
  follows as an instruction rather than treating as data. See `ADR-SEC-009`.
- **Erasure that does not erase.** "Delete everything" leaving recoverable user
  data behind.
- **Supply chain.** A dependency, build step, or release artifact that is not
  what it claims to be.

## Out of scope

To save your time:

- **Findings from automated scanners with no demonstrated impact.** Tell us what
  an attacker could actually do.
- **Vulnerabilities requiring an already-compromised machine.** The local vault
  is encrypted at rest, but an attacker with your OS user session and keychain
  is inside the trust boundary by design.
- **The installer being unsigned on Windows.** Known and deliberate — we are not
  yet a registered company and cannot obtain a code-signing certificate. A
  documented trade-off rather than a finding. Build provenance attestation is
  planned for public release and is NOT in place yet.
- **Denial of service against a user's own local vault.**

## Supported versions

The project is pre-1.0 and under active development. Only the latest release on
`main` is supported. There are no backported security fixes to older builds —
fixes land in the next release.

## Our side of the deal

We run the following on every change, so that a report is met with a codebase
that is actually maintained rather than one nobody has looked at:

- Secret scanning and push protection, plus full-history scanning
- Dependency advisory, licence, and source-origin checks on every pull request
- Static analysis (CodeQL)
- Encryption-at-rest and prompt-injection tests as blocking CI gates
- All CI actions pinned to commit hashes

Thank you for helping keep people's memories private.
