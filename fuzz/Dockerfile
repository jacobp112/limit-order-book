# Linux + nightly environment for cargo-fuzz (libFuzzer on Windows is
# experimental). Build from the repository root:
#   docker build -t lob-fuzz fuzz
FROM rustlang/rust:nightly-bookworm
RUN cargo install cargo-fuzz --locked
WORKDIR /work/fuzz
