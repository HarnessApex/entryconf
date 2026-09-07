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

## Editing (0.3.0)

A settings UI or CLI can also *write* configuration through the same library,
without losing track of where values come from (SPEC §10):

```go
snap, _ := entryconf.Open("envs/deploy")             // effective tree + every source document
origin, _ := snap.Inspect("/log/level")              // -> entrypoint.toml, "/log/level", "${LOG_LEVEL:-info}", variables: LOG_LEVEL (default)
plan, _ := snap.Plan([]entryconf.Edit{{             // one explicitly named JSON document, a batch of edits
    Document: "db.json", Op: "set", Pointer: "/pool/max", Value: 20,
}})
// plan.Candidate is the tree a commit would produce — validate it however you like first
receipt, err := plan.Commit(nil)                     // atomic, revision-checked, E_STALE_PLAN on conflict
```

The rules, in short: only **JSON** documents are writable (YAML and TOML are
read as before but never rewritten, so their comments survive); an edit always
names its document, and a JSON pointer within it; a **literal** value loads
back exactly as given (`$` and leading `@` are escaped for you), while an
**expression** is written verbatim; a plan is bound to the revisions of every
file and variable it depends on and a commit refuses to write if any changed;
one commit writes one file, atomically, under an advisory lock shared by every
implementation. The written file is re-serialized in a normalized form (sorted
keys, two-space indent). See [docs/EDITING.md](docs/EDITING.md) for the full
API tour and the integration handoff.

## Cross-language by construction

The normative document is [SPEC.md](SPEC.md). Correctness is defined by the
language-neutral fixture suites in [testdata/](testdata/): an implementation is
conformant iff it passes every case. Implementations are intended to be thin.

## Status

`v0.3.0` — spec and conformance suites are in place, and all four
implementations (Go, Python, TypeScript, Rust) pass every read case in
[testdata/cases/](testdata/cases/) and every editing case in
[testdata/editcases/](testdata/editcases/). `Load(dir)` is unchanged from
0.2.0; the editing surface is additive.

## License

Apache-2.0 — see [LICENSE](LICENSE).
