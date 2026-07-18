# Alpine would not be sufficient
FROM rust:1.75-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --workspace --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
ARG BIN
COPY --from=build /src/target/release/$BIN /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]
