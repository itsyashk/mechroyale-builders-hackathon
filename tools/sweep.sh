#!/bin/sh
# Usage (inside dev container): [JOBS=4] sh tools/sweep.sh MATCHES SEED "name:params" ...
M=$1; S=$2; shift 2
J=${JOBS:-4}
for spec in "$@"; do
  name=${spec%%:*}; prm=${spec#*:}
  r=$(MECHBOT_PARAMS="$prm" /target/release/sim practice --bot /target/release/mechbot --matches $M --jobs $J --seed $S --quiet 2>&1 | grep -E "SCORE|kills")
  echo "$name s$S [$prm] $(echo $r | tr '\n' ' ')"
done
