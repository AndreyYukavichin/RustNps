# syntax=docker/dockerfile:1.7

FROM rust:1.86-bookworm AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential ca-certificates cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
RUN cargo fetch --locked

COPY src ./src
COPY web ./web
COPY conf ./conf

RUN cargo build --release --locked --bin rnps --bin rnpc

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /usr/sbin/nologin rustnps \
    && mkdir -p /etc/rustnps \
    && chown -R rustnps:rustnps /etc/rustnps /home/rustnps

COPY --from=builder /app/target/release/rnps /usr/local/bin/rnps
COPY --from=builder /app/target/release/rnpc /usr/local/bin/rnpc
COPY --from=builder /app/conf /etc/rustnps
COPY --from=builder /app/web /etc/rustnps/web
COPY docker/entrypoint.sh /usr/local/bin/rustnps-entrypoint.sh

RUN chmod 0755 /usr/local/bin/rustnps-entrypoint.sh /usr/local/bin/rnps /usr/local/bin/rnpc

ENV RUSTNPS_CONF_DIR=/etc/rustnps

WORKDIR /etc/rustnps

EXPOSE 8081 8024 80 443

ENTRYPOINT ["/usr/local/bin/rustnps-entrypoint.sh"]
CMD ["rnps"]