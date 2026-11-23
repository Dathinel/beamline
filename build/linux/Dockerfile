# Compila y prueba BeamLine para Linux (build\app-linux.ps1): Rust fijado como en .entorno, .NET SDK fijado como en
# global.json, clang para Native AOT y Xvfb con lo que Uno (Skia sobre X11) necesita para la autoprueba.
FROM rust:1.100.0-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang zlib1g-dev xvfb xauth libx11-6 libxrandr2 libxi6 libxcursor1 libxext6 libxrender1 libice6 libsm6 \
        libfontconfig1 fonts-dejavu-core libgl1 libgl1-mesa-dri libegl1 zenity ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN curl -sSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
    && bash /tmp/dotnet-install.sh --version 10.0.403 --install-dir /usr/share/dotnet \
    && ln -s /usr/share/dotnet/dotnet /usr/local/bin/dotnet && rm /tmp/dotnet-install.sh
ENV DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1 CARGO_BUILD_JOBS=8
