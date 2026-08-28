---
name: Spec change
about: SPEC.md is wrong, ambiguous, or silent about a behavior
title: "[spec] "
labels: ["spec"]
---

<!--
If the spec already answers this and an implementation gets it wrong, that is a
conformance failure, not a spec change — use the other template.
Security issues go through GitHub Security Advisories; see SECURITY.md.
-->

## Section

Which part of `SPEC.md` does this touch?

- Section: <!-- e.g. §6 Interpolation, §2 Data model / TOML, §5 Includes -->
- Current wording (quote it, or write "silent — nothing covers this case"):

> 

## The case the spec does not settle

The smallest input whose result is unclear, and why. If implementations already
disagree, say what each one does today.

```

```

## Proposed wording

Concrete replacement or added text, written as it would appear in `SPEC.md` —
normative, RFC 2119 keywords where they belong. Name the `E_*` code if the
answer is an error (a *new* code also needs a row in the SPEC §7 table and in
`testdata/errors.json`).

> 

## Fixture that would pin it

Every normative MUST needs at least one fixture, and every error code at least
one failure case. Sketch it:

- Case directory (numbered, kebab-case): `NN-some-behavior/`
- `config/` contents (`EC_`-prefixed variables so the real environment cannot
  leak in):

```

```

- Expectation — `expected.json` tree, or the single `E_*` code for
  `expected_error.txt`:

```

```

## Impact

- [ ] Behavior change: existing configs would load differently, or an
      implementation would have to change
- [ ] Clarification only: pins behavior all implementations already have
- [ ] Would this change an existing `E_*` code's meaning? (Codes are public
      contract — see `CONTRIBUTING.md`)

Which design invariants does this interact with (conventions over flexibility,
unordered `.env` peers, fail loudly, inert substitution output, strict `$`)?

## Willing to do the work?

Behavior changes ship in **one PR**: `SPEC.md` + fixture + all four
implementations.

- [ ] I can open that PR
- [ ] I can write the spec wording and fixture but need help with the
      implementations
- [ ] Reporting only
