#!/bin/sh
# Head-to-head tournament of bot variants on identical seeds (run INSIDE the dev container).
# Each variant: "name|/path/to/mechbot|k=v,k=v" (bin may be empty = /target/release/mechbot).
# Env: MATCHES (96) SEEDS ("100 200") MODELS ("v1 v3") JOBS (7) SIM (/target/release/sim)
# Uses --budget-ms 1000 so CPU contention never makes answers late (results = pure behaviour).
# Output: one line per (variant, model, seed) and a summary sorted by mean score.
M=${MATCHES:-96}; SEEDS=${SEEDS:-"100 200"}; MODELS=${MODELS:-"v1 v3"}; J=${JOBS:-7}
SIM=${SIM:-/target/release/sim}
OUT=${OUT:-/tmp/tourney.$$}; mkdir -p "$OUT"
for spec in "$@"; do
  name=$(echo "$spec" | cut -d'|' -f1); bin=$(echo "$spec" | cut -d'|' -f2); prm=$(echo "$spec" | cut -d'|' -f3)
  [ -z "$bin" ] && bin=/target/release/mechbot
  for mdl in $MODELS; do for s in $SEEDS; do
    r=$(SIM_HOUSE=$mdl MECHBOT_PARAMS="$prm" "$SIM" practice --bot "$bin" --matches $M --jobs $J --seed $s --budget-ms 1000 --quiet 2>&1 | grep -E "SCORE" | head -1)
    sc=$(echo "$r" | grep -oE "[0-9]+\.[0-9]+" | head -1)
    echo "$name $mdl s$s $sc" | tee -a "$OUT/res.txt"
  done; done
done
echo "=== summary (mean over models x seeds; v1 / v3 means) ==="
awk '{n[$1]++; t[$1]+=$4; if($2=="v1"){a[$1]+=$4;na[$1]++} else {b[$1]+=$4;nb[$1]++}}
  END{for(k in n) printf "%-22s mean %.3f  v1 %.3f  v3 %.3f\n", k, t[k]/n[k], (na[k]?a[k]/na[k]:0), (nb[k]?b[k]/nb[k]:0)}' "$OUT/res.txt" | sort -k3 -nr
