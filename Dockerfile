FROM rust:1-slim-bookworm AS build
WORKDIR /src
# Cache dependencies separately from source changes
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && touch src/lib.rs \
 && cargo build --release && rm -rf src
COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* && useradd -r -u 10001 cloudcache
COPY --from=build /src/target/release/cloud-cache /usr/local/bin/cloud-cache
USER cloudcache
EXPOSE 8080 7379
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/cloud-cache"]
