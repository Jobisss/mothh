FROM rust:1-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --locked --release -j 2 -p server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home mothh \
    && mkdir -p /var/lib/mothh/anexos \
    && chown -R mothh:mothh /var/lib/mothh
COPY --from=build /app/target/release/server /usr/local/bin/server
USER mothh
ENV MOTHH_ENDERECO=0.0.0.0:3000 \
    MOTHH_ANEXOS=/var/lib/mothh/anexos
EXPOSE 3000
CMD ["/usr/local/bin/server"]
