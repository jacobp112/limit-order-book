# limit-order-book

A deterministic in-memory limit order book and matching engine in Rust, with
price-time priority, limit/market/IOC orders, cancel and amend, and exact
replay from a command journal.

Work in progress. See:

- [Requirements, invariants and error semantics](docs/requirements.md)
- [Design and data structures](docs/design.md)
- [Testing and benchmark strategy](docs/testing.md)
- [Backlog](docs/backlog.md)

## Build

```
cargo test --workspace
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
