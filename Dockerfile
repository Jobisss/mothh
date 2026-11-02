FROM denoland/deno:bin-2.9.8 AS deno

FROM rust:1-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libglib2.0-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --locked --release -j 2 -p server -p music-worker

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates ffmpeg python3 python3-venv \
       libglib2.0-0 libasound2 libssl3 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home mothh \
    && mkdir -p /var/lib/mothh/anexos \
    && chown -R mothh:mothh /var/lib/mothh
ARG YT_DLP_VERSION=2026.10.26
ARG POT_PROVIDER_VERSION=2.0.2
RUN python3 -m venv /opt/yt-dlp \
    && /opt/yt-dlp/bin/pip install --no-cache-dir \
       "yt-dlp[default]==${YT_DLP_VERSION}" \
       "bgutil-ytdlp-pot-provider==${POT_PROVIDER_VERSION}"
COPY infra/music/yt_dlp.py /usr/local/bin/mothh-yt-dlp
RUN chmod 755 /usr/local/bin/mothh-yt-dlp
COPY --from=deno /deno /usr/local/bin/deno
COPY --from=build /app/target/release/server /usr/local/bin/server
COPY --from=build /app/target/release/music-worker /usr/local/bin/music-worker
USER mothh
ENV MOTHH_ENDERECO=0.0.0.0:3000 \
    MOTHH_ANEXOS=/var/lib/mothh/anexos \
    MOTHH_MUSIC_WORKER= \
    MOTHH_YT_DLP=/usr/local/bin/mothh-yt-dlp \
    MOTHH_FFMPEG=/usr/bin/ffmpeg
EXPOSE 3000
CMD ["/usr/local/bin/server"]
