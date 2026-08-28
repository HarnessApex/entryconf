# Security Policy

## Reporting a vulnerability

**Report privately, never in a public issue or pull request.**

Use GitHub Security Advisories on this repository:
[Report a vulnerability](https://github.com/HarnessApex/entryconf/security/advisories/new)
(repository → **Security** → **Advisories** → **Report a vulnerability**). That
channel is private between you and the maintainers until an advisory is
published.

If GitHub advisories are unavailable to you, open a public issue that says only
that you have a security report and asks for a private channel — no details.

Please include, as far as you can:

- affected implementation(s) and version (`go/`, `python/`, `ts/`, `rust/`, or
  the spec itself) and the commit you tested;
- a minimal config directory that reproduces the issue;
- what happens versus what the spec (`SPEC.md`) requires;
- the impact you believe it has.

Do not include working exploits against third-party systems, and do not test
against infrastructure you do not own.

## What counts

entryconf loads local configuration files at startup and is meant to fail
loudly. Things we treat as security issues include: reading or writing files
outside the config directory in a way the spec does not describe, an `@file:`
path or include chain causing unbounded resource use, a crafted config file
causing memory unsafety or arbitrary code execution in an implementation, and
leaking variable values into places the spec does not put them.

A *conformance* difference with no security impact — an implementation
disagreeing with `SPEC.md` or with another implementation — is a normal bug.
Please file it as a public conformance-failure issue with a fixture instead;
see `CONTRIBUTING.md`.

## Process and scope

We will acknowledge a report, confirm or dispute it, and coordinate a fix and
an advisory with you. While the spec is `0.x` there are no long-term
support branches: fixes land on `main`, and the advisory names the affected
implementations and commits. Credit is given unless you ask otherwise.
