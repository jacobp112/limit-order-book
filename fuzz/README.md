# Fuzzing

Three [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets:

| Target | Input | Fails if |
|---|---|---|
| `commands` | bytes decoded (via `arbitrary`) into a command stream | any panic; any invariant violation after any command; replay or snapshot-split replay differs |
| `journal` | arbitrary text through the journal parser | parser panics or reports a bad line number; parsed commands do not print/re-parse identically or violate invariants when run |
| `snapshot` | arbitrary bytes through `Snapshot::decode` | decoder panics; an accepted snapshot is not canonical, does not restore to the same state, has a broken book, or breaks invariants when swept by market orders |

Structured commands mostly use prices near a common mid and small
quantities so orders cross, queue and partially fill, but can also carry any
`i64` price or `u64` quantity. Cancels and amends target recently issued
ids (live, filled or cancelled) or any raw id.

The harness logic lives in `src/lib.rs` and is exercised on stable by
`cargo test --release` in this directory (seed corpus, random bytes, mangled
journals, bit-flipped snapshots). Coverage-guided fuzzing needs nightly and
libFuzzer, so it runs in a Linux container.

## Running

From the repository root:

```
docker build -t lob-fuzz fuzz
docker run --rm -v "$PWD":/work -v lob-fuzz-target:/work/fuzz/target lob-fuzz \
  bash -c "mkdir -p corpus/journal && cargo fuzz run --features libfuzzer \
    journal corpus/journal seeds/journal -- -max_total_time=600"
```

libFuzzer refuses to start if an explicitly named corpus directory does not
exist, hence the `mkdir`. Replace `journal` with `commands` or `snapshot`
(those have no seed directory, so drop `seeds/journal`). `corpus/` and `artifacts/` are ignored by Git. A crash is saved
under `artifacts/<target>/`; reproduce it with
`cargo fuzz run --features libfuzzer <target> artifacts/<target>/<file>`.

