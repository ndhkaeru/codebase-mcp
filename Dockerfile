FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --create-home --uid 10001 codebase
COPY --from=builder /app/target/release/codebase-mcp /usr/local/bin/codebase-mcp
USER codebase
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/codebase-mcp"]
