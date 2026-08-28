---
name: Conformance failure
about: An implementation disagrees with SPEC.md, a fixture, or another implementation
title: "[conformance] "
labels: ["conformance"]
---

<!--
Not a vulnerability report. Security issues go through GitHub Security
Advisories — see SECURITY.md.
-->

## Implementation and version

- Implementation: <!-- go / python / ts / rust — or "spec" if the spec itself is unclear -->
- Version / commit: <!-- e.g. 0.1.0, or the entryconf commit SHA you tested -->
- Language toolchain: <!-- e.g. Go 1.23, Python 3.13, Node 22.18, rustc 1.83 -->
- OS: <!-- e.g. ubuntu-latest, macOS 15 -->

## Minimal config directory

Trim it to the smallest thing that still shows the problem. Include every file,
with its path and full contents.

```
config/
  entrypoint.yaml
  app.env
```

<details><summary>config/entrypoint.yaml</summary>

```yaml

```

</details>

<details><summary>config/app.env</summary>

```

```

</details>

Process environment set for the load (the `procenv.json` equivalent), if any:

```json
{}
```

## Expected

The tree you expect (as `expected.json` would hold it), **or** the single `E_*`
code you expect:

```json

```

## Actual

What the implementation produced — tree, or `E_*` code, or an uncoded
crash/panic (paste the message):

```

```

## Spec reference

Which part of `SPEC.md` says the expected result is right (e.g. "§6
whole-value typing", "§4 process env overrides")? If the spec does not say,
file a spec-change issue instead.

## Reproduces via the dump command?

Please confirm with the implementation's dump CLI, which prints the tree as
JSON on stdout (exit 0) or the bare `E_*` code on stderr (exit 1):

- `cd go && go run ./cmd/entryconf dump <dir>`
- `python -m entryconf <dir>`
- `cd ts && node src/cli.ts <dir>`
- `cd rust && cargo run --quiet --bin entryconf-dump -- <dir>`

- [ ] Yes, it reproduces via the dump command (output pasted above)
- [ ] No — it only reproduces through the library API (say how you called it)
- [ ] Not tried

Other implementations, if you checked them (or paste
`python3 tools/crosscheck/crosscheck.py -case <name> -v`):

```

```

## Fixture

A fixture is the most useful form of this report. If you can, propose one:

- Case directory name (numbered, kebab-case): `NN-some-behavior/`
- Variables use the `EC_` prefix so real environment variables cannot leak in.

- [ ] I can open a PR adding this fixture (spec-first rule: `SPEC.md` +
      fixture + all implementations in one PR — see `CONTRIBUTING.md`)
