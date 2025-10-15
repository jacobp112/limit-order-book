# limit-order-book

A deterministic in-memory limit order book and matching engine in Rust, with
price-time priority, limit/market/IOC orders, cancel and amend, and exact
replay from a command journal.

Work in progress. See:

- [Requirements, invariants and error semantics](docs/requirements.md)
- [Design and data structures](docs/design.md)
- [Testing and benchmark strategy](docs/testing.md)
- [Benchmark results and method](docs/benchmarks.md)
- [Backlog](docs/backlog.md)
- [Explainer: step through matching in the browser](explainer/README.md)

## Build

```
cargo test --workspace
```

## Replay a journal

```
cargo run -p lob-cli -- replay examples/journals/session.journal
```

Prints each command with its events, then the event and state digests.
`--save <file>` writes the final snapshot and `--from <file>` starts from
one; `--quiet` prints only the summary.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
