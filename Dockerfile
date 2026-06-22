FROM rust:latest AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo

RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && mkdir -p migrations/20250619000001_create_all_tables \
    && touch migrations/20250619000001_create_all_tables/up.sql migrations/20250619000001_create_all_tables/down.sql \
    && cargo build --release 2>/dev/null || true

COPY . .
RUN touch src/main.rs && cargo build --release

FROM debian:stable-slim
RUN apt-get update -qq && apt-get install -y -qq ca-certificates sqlite3 curl && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/password-manager-server /usr/local/bin/password-manager-server
COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/admin_cli /usr/local/bin/admin_cli

EXPOSE 53971
CMD ["password-manager-server"]
