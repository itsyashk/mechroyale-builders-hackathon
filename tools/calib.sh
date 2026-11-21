#!/bin/sh
# Usage (dev container): sh tools/calib.sh MATCHES SEED "ENV=.. ENV=.." ...
# Scores the d547b5b and 73be899 parameter sets under each house-model setting.
M=$1; S=$2; shift 2
D547="finish=0,kite_light=0,gun_lo=18,gun_hi=55,laser_lo=45,laser_hi=140,light_lo=10,light_hi=32"
for e in "$@"; do
  for spec in "73be:" "d547:$D547"; do
    name=${spec%%:*}; prm=${spec#*:}
    r=$(env $e MECHBOT_PARAMS="$prm" /target/release/sim practice --bot /target/release/mechbot --matches $M --jobs ${JOBS:-4} --seed $S --quiet 2>&1 | grep -E "SCORE|kills" | tr '\n' ' ')
    echo "[$e] $name $(echo $r | grep -o 'SCORE mean(16-place) = [0-9.]*' | cut -d' ' -f4) $(echo $r | grep -o 'kills [0-9.]*  survival [0-9.]*s')"
  done
done
