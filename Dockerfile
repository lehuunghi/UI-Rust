FROM rust:1.90-bookworm AS build
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
COPY migrations ./migrations
COPY static ./static
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* && useradd --system --uid 10001 app
WORKDIR /app
COPY --from=build /app/target/release/ui-rust /usr/local/bin/ui-rust
COPY static ./static
USER app
ENV BIND=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["ui-rust"]
CMD ["serve"]
