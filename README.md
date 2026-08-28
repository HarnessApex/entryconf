# entryconf

**One entrypoint. Plain files. Any language.**

entryconf is a convention for loading a directory of config files into a single
tree — designed to behave *identically* in every language that implements it.
No manifest, no merge-order rules, no framework. A config directory looks like
this:

```
envs/deploy/
  entrypoint.toml     ← the single entrypoint
  deploy.env          ← variables (all *.env files are loaded)
  db.json             ← referenced from the entrypoint
```

```toml
# entrypoint.toml
app = "checkout"

[database]
conn     = "@file:db.json"        # graft another file's tree here
password = "$DB_PASSWORD"         # from deploy.env (or process env)

[log]
level = "${LOG_LEVEL:-info}"      # with a default
```

```python
cfg = entryconf.load("envs/deploy")
```

## The whole convention

1. **One entrypoint per directory** — `entrypoint.json` / `.yaml` / `.yml` / `.toml`. Zero or two+ is an error.
2. **All `*.env` files in the directory are loaded.** They are peers with no ordering, so the same variable in two files is an error. The process environment overrides them (the CI/deploy escape hatch).
3. **`"@file:path"` grafts another file** (JSON/YAML/TOML, mixed freely) into that position. Paths are relative to the file containing the reference. Cycles are errors.
4. **`$VAR` / `${VAR}` / `${VAR:-default}` interpolate variables** after the tree is assembled. A string that is exactly one reference keeps its scalar type (`"${PORT}"` → `8080`, not `"8080"`). A missing variable with no default is an error.
5. **Everything fails loudly at load time.** No file is silently skipped; no partial config is ever returned.

Environments share *structure* through `@file:` includes and vary through their
own `.env` values — that's the layering story, in three levels anyone can
recite: `process env > .env files > ${VAR:-default}`.

## Cross-language by construction

The normative document is [SPEC.md](SPEC.md). Correctness is defined by the
language-neutral fixture suite in [testdata/](testdata/): an implementation is
conformant iff it passes every case. Implementations are intended to be thin
(a few hundred lines each).

## Status

`v0.2.0` — spec and conformance suite are in place, and all four
implementations (Go, Python, TypeScript, Rust) pass every case in
[testdata/cases/](testdata/cases/).

## License

Apache-2.0 — see [LICENSE](LICENSE).
