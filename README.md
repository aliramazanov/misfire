# misfire

**Did this diff make your tests misfire?** Caught before the review, not after.

A misfire is a round that chambers, fires, and delivers nothing. That is what a test
becomes when its assertions are deleted: it still runs, still reports green, and verifies
nothing at all.

```console
$ misfire --base main
misfire: 5 findings in 2 files

  payment.rs:4     MF106  refund_rejects dropped the expected message, so it now passes on any failure
  payment.rs:10    MF105  1 assertion in refund_succeeds no longer stops the test on failure, so what follows runs on bad state
  user_test.go:5   MF101  2 assertions removed from TestCreateUser
  user_test.go:11  MF102  TestDeleteUser now calls t.Skip
  user_test.go:16  MF103  TestRefund added with 0 assertions

  5 findings. Review the test changes before merging.
```

## Why

A passing build is supposed to be evidence. It stops being evidence the moment a change
removes the assertions instead of fixing the code, which is now a routine failure mode in
AI-authored pull requests. Linters cannot see it: the file parses, the tests run, coverage
barely moves. The signal exists only in the diff.

misfire parses the old blob and the new blob and compares the two sets of test functions.
It never pattern-matches diff text, which is why a `gofmt` pass that rewrites twenty lines
reports nothing.

## Rules

| ID | Fires when |
|---|---|
| MF101 | A test survives the change with fewer assertions than before |
| MF102 | A test stops running: `t.Skip`, `#[ignore]`, or a new `#[cfg(feature = "…")]` gate |
| MF103 | A new test is added with no assertions at all |
| MF104 | An assertion is replaced with a broader one, such as `assert.Equal` to `assert.NotNil` |
| MF105 | A failure stops being fatal: `require` to `assert`, `t.Fatal` to `t.Error`, `assert!` to `debug_assert!` |
| MF106 | An expected-failure check is dropped or widened, including `#[should_panic(expected = "…")]` losing its message |
| MF107 | A test file is deleted while its siblings changed (off by default) |
| MF108 | A test that used to run no longer runs |
| MF109 | Go's `TestMain` no longer calls `m.Run()`, so nothing in the package executes |
| MF110 | A `//go:build` constraint now excludes a test file |

The quiet ones are worth naming. `require.NoError` calls `FailNow` and `assert.NoError`
calls `Fail`, so swapping them on a precondition means every later assertion runs against
state you already know is wrong. `debug_assert!` is compiled out of release builds.
Gutting `TestMain`, tagging a file, or gating a test behind a feature removes tests from
the run entirely while `go test` still prints `ok`.

## Languages

Go and Rust, both by AST. Multi-line assertions, renamed `*testing.T` receivers, subtests
in `t.Run`, fuzz targets, examples with `// Output:`, and tests inside `#[cfg(test)] mod`
all work.

Assertions behind helpers are counted, including helpers in sibling files, because real
suites write `expectSuccess(out, err, t)` rather than a bare assertion. A construct the
grammar cannot read costs that one test, not the file, and misfire says which.

## Install

```bash
cargo install misfire
```

Requires Rust 1.88 or newer. One static binary, no runtime.

## Usage

```bash
misfire                          # compare against origin/main
misfire --base HEAD~1            # compare against a ref
misfire --json                   # machine-readable
misfire --github                 # inline pull request annotations
misfire --sarif                  # SARIF 2.1.0 for code scanning
misfire --min-confidence high    # only high-confidence findings fail the build
misfire --explain                # why each test did or did not produce a finding
```

Also `--path DIR` to run from outside the repository, `--baseline FILE` to apply a recorded
one, `--file-deleted-rule` to enable MF107, `-v` and `-vv` for phase timings and rule
decisions, `--log-format json` for structured events, `--metrics` and `--timings` for counts
and per-phase totals, and `--otlp-endpoint URL` to export spans on a build with
`--features otel`. Narration goes to stderr, so `--json` and `--sarif` stay pipeable, and a
default run writes nothing there at all.

Exit codes: `0` clean, `1` findings at or above `--min-confidence`, `2` misfire could not
run. A base ref it cannot reach is an error, never an empty report. In CI, `actions/checkout`
clones with `fetch-depth: 1`; misfire fetches what it needs and fails loudly if it cannot.

## Configuration

Optional `misfire.toml` at the repository root. Every project has assertion helpers no
default list can know about.

```toml
[go]
assertion_patterns = ["testutil.*", "expect*"]
fatal_patterns = ["testutil.Must*"]

[rust]
assertion_patterns = ["assert_eq_printed"]
assertion_methods = ["assert*", "expect*"]

[rules]
disabled = ["MF104"]

[rules.confidence]
MF103 = "medium"
```

Patterns are globs matched against the whole call name. An unrecognised key is an error,
not a silently ignored default. A rule that is right about the mechanism but wrong about
the stakes for your project can be demoted rather than switched off.

## Adopting on an existing repository

Record what is already there, then gate only what comes next:

```bash
misfire --write-baseline
git add .misfire-baseline.json
```

Baseline entries key on file, rule and test name, never line number, so unrelated edits do
not make them go stale. To accept one finding permanently, say so where it happens:

```go
// misfire:allow MF101 the assertions moved to the integration suite
```

That suppresses one rule on one test, not the line or the file. A removed test has no body
left to annotate, so MF107 and MF108 are anchored to the file that lost it.

## GitHub Actions

```yaml
- uses: actions/checkout@v4
- uses: aliramazanov/misfire@v0
```

## What it does not do

- It is not a linter. It reports on what the change did, never on code you did not touch.
- It does not judge whether a test is good, only whether the change made it weaker.
- It cannot tell a legitimate refactor from a cover-up. It surfaces, you decide.

## License

MIT
