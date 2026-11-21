#!/bin/sh
# FFA (house bots fight everyone) scores for the d547b5b / 73be899 param sets and the example bot.
M=$1; S=$2; shift 2
D547="finish=0,kite_light=0,gun_lo=18,gun_hi=55,laser_lo=45,laser_hi=140,light_lo=10,light_hi=32"
for e in "$@"; do
  for spec in "73be:" "d547:$D547"; do
    name=${spec%%:*}; prm=${spec#*:}
    r=$(env $e MECHBOT_PARAMS="$prm" /target/release/sim ffa --bots /target/release/mechbot --ext-slots 1 --matches $M --jobs ${JOBS:-4} --seed $S --quiet 2>&1 | grep -E "mean\(16-place\)" | head -1)
    echo "[$e] $name $r"
  done
  r=$(env $e /target/release/sim ffa --bots "python3 -u kit/python/example_bot.py" --ext-slots 1 --matches 32 --jobs ${JOBS:-4} --seed $S --quiet 2>&1 | grep -E "mean\(16-place\)" | head -1)
  echo "[$e] example $r"
done
