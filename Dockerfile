from ekidd/rust-musl-builder as build
workdir /usr/src/ligmir

run sudo chown -R . && mkdir -p src/ && touch src/lib.rs
copy Cargo.toml Cargo.lock ./
run cargo build --locked --lib --release --target x86_64-unknown-linux-musl

copy src ./src
run cargo install --locked --path . --root . --target x86_64-unknown-linux-musl


from scratch

copy --from=build /usr/src/ligmir/bin/ligmir /
entrypoint ["/ligmir"]
