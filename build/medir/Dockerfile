# Imagen para simular varios PC con Docker (build\medir\medir.ps1): Rust fijado como en .entorno, tc (netem) e iroh-relay.
FROM rust:1.100.0-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends iproute2 && rm -rf /var/lib/apt/lists/*
ENV CARGO_BUILD_JOBS=8
RUN cargo install iroh-relay --version 1.5.0 --features server --locked
