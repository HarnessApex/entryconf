# Conformance suite

Each directory under `cases/` is one test case. An implementation is
conformant iff every case passes. See SPEC.md §8 for the harness contract.

```
cases/<name>/
  config/               the config directory passed to Load()
  procenv.json          optional: process env to set for this case
  expected.json         success cases: expected tree (structural equality)
  expected_error.txt    failure cases: expected error code
```

A minimal harness, in pseudocode:

```
for case in cases/*:
    env = read(case/procenv.json) if exists else {}
    with process_env(env):                    # case-named vars otherwise unset
        result = try Load(case/config)
    if exists(case/expected_error.txt):
        assert result is error with that code
    else:
        assert result == parse(case/expected.json)   # numbers numerically
```

## Editing suite

Each directory under `editcases/` is one editing case (SPEC §11). The same
harness contract applies, plus: the case runs against a **copy** of `config/`,
and afterwards every file the case did not write must be byte-identical to the
copy (or to what `before_commit` overwrote it with), with no lock or temporary
file left behind.

```
editcases/<name>/
  config/               the initial config directory (copied before the run)
  procenv.json          optional: process env to set for this case
  request.json          {"inspect": ptr} | {"documents": true} | {"edits": [...], "before_commit"?: {...}}
  expected.json         success: {"origin"} | {"documents"} | {"tree", "affected", "documents"}
  expected_error.txt    failure: expected error code
```

```
for case in editcases/*:
    work = copy(case/config)
    env = read(case/procenv.json) if exists else {}
    with process_env(env):
        snap = Open(work)
        if "inspect" in request:    result = snap.Inspect(request.inspect)
        elif "documents" in request: result = snap.Documents (format, writable, grafts)
        else:
            plan = snap.Plan(request.edits)
            apply request.before_commit (write files / change env)
            receipt = plan.Commit()
            result = {tree: Load(work), affected: plan.Affected, documents: {plan.Document: parse(plan.After)}}
            assert Load(work) == plan.Candidate
    compare result with expected.json or the error code with expected_error.txt
    assert every other file in work is unchanged and nothing was added
```

`testdata/errors.json` says which suite covers each code: the read suite by
default, `"suite": "editcases"` for the editing codes, and `"suite": "unit"`
for `E_LOCKED` and `E_WRITE`, which depend on state a fixture cannot carry and
are covered by each implementation's unit tests.
