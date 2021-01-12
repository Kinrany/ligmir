FROM rust:1.49 as builder

WORKDIR /ligmir_build
COPY Cargo.toml Cargo.lock ./
# compile dependencies
RUN mkdir src && touch src/lib.rs && cargo build --release --lib && rm src/lib.rs
COPY src/* ./src/
RUN cargo build --release

FROM debian:latest

WORKDIR /ligmir_run
COPY --from=builder /ligmir_build/target/release/ligmir ./
ENTRYPOINT /ligmir_run/ligmir
