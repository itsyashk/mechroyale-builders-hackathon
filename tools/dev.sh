#!/bin/sh
# Run a command inside the Linux dev container with the repo mounted at /work.
# Build artifacts live in the docker volume "mech-target" (mounted at /target).
# Usage: tools/dev.sh cargo build --release -p sim
REPO="$(cd "$(dirname "$0")/.." && pwd)"
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) REPO="$(cygpath -w "$REPO")";; esac
exec docker run --rm -i -v "$REPO:/work" -v mech-target:/target -v mech-cargo:/usr/local/cargo/registry mech-dev "$@"
