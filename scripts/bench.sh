#!/bin/sh
# Fair speed comparison on one URL: SDM (several connection counts) vs aria2 vs curl.
# Rounds alternate between tools so network noise hits everyone equally; prints medians.
#   scripts/bench.sh <url> [rounds]     needs target/release/sdm-cli (cargo build --release); aria2c/curl optional
set -u
url=${1:?usage: bench.sh <url> [rounds]}
rounds=${2:-3}
root=$(cd "$(dirname "$0")/.." && pwd)
sdm=${SDM_CLI:-$root/target/release/sdm-cli}
[ -x "$sdm" ] || { echo "Build first: cargo build --release" >&2; exit 1; }
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
results="$tmp/results"; : > "$results"
now() { date +%s.%N; }

measure() { # label command...
  label=$1; shift
  rm -rf "$tmp/dl"; mkdir -p "$tmp/dl"
  s=$(now); "$@" >/dev/null 2>&1; rc=$?; e=$(now)
  size=$(find "$tmp/dl" -type f ! -name '*.part*' ! -name '*.aria2' -printf '%s\n' | head -1)
  if [ "$rc" -eq 0 ] && [ -n "${size:-}" ]; then
    echo "$label|$(echo "$s $e $size" | awk '{printf "%.2f", ($3/1048576)/($2-$1)}')" >> "$results"
  else
    echo "  ($label failed)" >&2
  fi
}

r=0
while [ "$r" -lt "$rounds" ]; do
  r=$((r+1)); echo "round $r/$rounds ..." >&2
  for c in 1 8 16 32; do measure "sdm -c $c" "$sdm" "$url" -d "$tmp/dl" -c "$c"; done
  command -v aria2c >/dev/null 2>&1 && measure "aria2c -x16 -s16" aria2c -q -x16 -s16 -k1M -d "$tmp/dl" -o out.bin --allow-overwrite=true "$url"
  command -v curl   >/dev/null 2>&1 && measure "curl (1 conn)" curl -sL -o "$tmp/dl/out.bin" "$url"
done

echo "Median MB/s over $rounds round(s):"
cut -d'|' -f1 "$results" | awk '!seen[$0]++' | while read -r label; do
  vals=$(grep -F "$label|" "$results" | cut -d'|' -f2 | sort -n | tr '\n' ' ')
  med=$(echo "$vals" | tr ' ' '\n' | grep . | awk '{x[NR]=$1} END {print x[int((NR+1)/2)]}')
  printf "  %-18s %8s   (all: %s)\n" "$label" "$med" "$vals"
done
