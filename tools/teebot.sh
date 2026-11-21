#!/bin/sh
# Runs a bot command while recording its observation stream to $TEE_DIR (for streamstats.py).
d=${TEE_DIR:-/tmp/obs}; mkdir -p "$d"
f="$d/obs_$$_$(date +%s%N).jsonl"
tee "$f" | "$@"
