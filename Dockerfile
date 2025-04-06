FROM lukemathwalker/cargo-chef:latest-rust-1 AS builder
RUN apt-get update && apt-get -y upgrade && apt-get install -y libclang-dev pkg-config git

COPY ./reth/Cargo.lock ./reth/Cargo.lock
COPY ./reth/Cargo.toml ./reth/Cargo.toml
COPY ./reth/crates ./reth/crates
COPY ./reth/bin ./reth/bin
COPY ./reth/examples ./reth/examples
COPY ./reth/testing ./reth/testing
COPY ./revm ./revm
COPY ./revm-inspectors ./revm-inspectors
COPY ./rbuilder ./rbuilder

WORKDIR /reth
RUN cargo build --release --bin reth
WORKDIR /rbuilder
RUN cargo build --release --bin reth-rbuilder    

FROM ubuntu:22.04 AS runtime
COPY --from=builder /reth/target/release/reth /usr/local/bin
COPY --from=builder /rbuilder/target/release/reth-rbuilder /usr/local/bin

COPY ./rbuilder/config-gwyneth-reth.toml /app/rbuilder/config-gwyneth-reth.toml
RUN echo '#!/bin/bash\nreth-rbuilder /app/rbuilder/config-gwyneth-reth.toml' > /app/start_rbuilder.sh && \
    chmod +x /app/start_rbuilder.sh
    
WORKDIR /app
# RUN reth

EXPOSE 30303 30303/udp 9001 8545 8546
ENTRYPOINT ["/usr/local/bin/reth"]