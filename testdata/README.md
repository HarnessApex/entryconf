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
