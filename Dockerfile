# mechfight/1 bot: giddybot (Rust port of the MIT-licensed giddy Go bot, Zig terrain raycast).
# mechbot (our own Rust + Zig bot) is still built so both stay buildable.
# Toolchains are pinned so finalist rebuilds are identical.
FROM rust:1.99.1-slim-bookworm AS build
ARG ZIG_VERSION=0.17.1
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates xz-utils \
 && rm -rf /var/lib/apt/lists/*
RUN curl -sSfL --retry 5 --retry-delay 3 \
      "https://ziglang.org/download/${ZIG_VERSION}/zig-x86_64-linux-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz \
 && mkdir -p /opt/zig && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1 \
 && rm /tmp/zig.tar.xz && /opt/zig/zig version
ENV ZIG=/opt/zig/zig
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY zig zig
RUN cargo build --release --locked -p mechbot -p giddybot \
 && strip target/release/mechbot target/release/giddybot

FROM debian:bookworm-slim
COPY --from=build /src/target/release/mechbot /usr/local/bin/mechbot
COPY --from=build /src/target/release/giddybot /usr/local/bin/giddybot
COPY crates/giddy/LICENSE-MIT-giddy /usr/share/doc/giddybot/LICENSE-MIT-giddy
ENTRYPOINT ["/usr/local/bin/giddybot"]
