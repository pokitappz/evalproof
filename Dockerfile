FROM rust:1.96-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY packages ./packages
COPY examples ./examples
RUN cargo build --locked --release --bin evalproof-entitlements

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/evalproof-entitlements /usr/local/bin/
USER 65532:65532
ENV BIND_ADDRESS=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["evalproof-entitlements"]
