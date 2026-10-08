FROM rust:1.99-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY locales ./locales
COPY static ./static
RUN cargo build --release --locked -j4

FROM postgres:17-bookworm AS pgtools

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates openssl rclone libpq5 libzstd1 liblz4-1 && rm -rf /var/lib/apt/lists/* && useradd --system --uid 10001 app
WORKDIR /app
COPY --from=build /app/target/release/ui-rust /usr/local/bin/ui-rust
COPY --from=pgtools /usr/lib/postgresql/17/bin/pg_dump /usr/lib/postgresql/17/bin/pg_restore /usr/local/bin/
COPY --from=pgtools /usr/lib/x86_64-linux-gnu/libpq.so.5* /usr/local/lib/
RUN ldconfig && mkdir -p /app/backups && chown app:app /app/backups
COPY static ./static
RUN chmod -R u=rwX,go=rX /app/static
USER app
ENV BIND=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["ui-rust"]
CMD ["serve"]
