# Dev/test image: Rust + Zig + Python, for running the simulator and checks in Linux.
FROM rust:1.99.1-slim-bookworm
ARG ZIG_VERSION=0.17.1
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates xz-utils python3 && rm -rf /var/lib/apt/lists/*
RUN curl -sSfL --retry 5 "https://ziglang.org/download/${ZIG_VERSION}/zig-x86_64-linux-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz \
 && mkdir -p /opt/zig && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1 && rm /tmp/zig.tar.xz
ENV ZIG=/opt/zig/zig CARGO_TARGET_DIR=/target
WORKDIR /work
