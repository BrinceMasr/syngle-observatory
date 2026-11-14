#!/usr/bin/env bash
# Builds what the evaluators run against on a boat box (never cargo on the PC or in WSL) and brings it back.
#
#   evals/build/boat.sh [--box NAME] [--steps "canvas engine data"] [--out DIR] [--hours N] [--size large] [--down]
#
# One start per box (boat allows 200 a day, the workspace gives a climber up to 4): the box stays up between builds, so a
# second build reuses cargo's target folder and takes minutes. `--down` deletes the box when the build has come back.
# The build lands in DIR (default C:\dev\scratch\obs-evals\<box>\build, as /mnt/c/dev/scratch/...): a Windows folder, because
# the evaluators run the Windows engine and Windows Chrome. See evals/README.md.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BOX="${BOX:-map-perf}"
STEPS="canvas engine data"
HOURS=8
SIZE=large
DOWN=0
OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --box) BOX="$2"; shift 2 ;;
    --steps) STEPS="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --hours) HOURS="$2"; shift 2 ;;
    --size) SIZE="$2"; shift 2 ;;
    --down) DOWN=1; shift ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done

BOAT="${BOAT_BUILD:-}"
if [ -z "$BOAT" ]; then
  for c in "$HOME/projects/Syngle-Ecosystem/agents/scripts/boat-build" "$REPO/../../agents/scripts/boat-build" "$REPO/../Syngle-Ecosystem/agents/scripts/boat-build"; do
    if [ -f "$c/box.mjs" ]; then BOAT="$c"; break; fi
  done
fi
[ -f "$BOAT/box.mjs" ] || { echo "boat-build not found: set BOAT_BUILD to agents/scripts/boat-build" >&2; exit 2; }
OUT="${OUT:-/mnt/c/dev/scratch/obs-evals/$BOX/build}"
box() { node "$BOAT/box.mjs" "$@"; }

if ! box list 2>/dev/null | awk '{print $1}' | grep -qx "$BOX"; then
  box up "$BOX" --size "$SIZE" --hours "$HOURS"
fi
box sync "$BOX" "$REPO"
box exec "$BOX" --cwd /home/user/w/syngle-observatory -- "STEPS='$STEPS' bash evals/build/on-box.sh"
# The point files are deterministic and 340 MB: they come back only when the data step ran.
EXCLUDE="--exclude=build/data"
case " $STEPS " in *" data "*) EXCLUDE="" ;; esac
box exec "$BOX" -- "cd /home/user/out && tar cf build.tar $EXCLUDE build && ls -l build.tar"
mkdir -p "$OUT"
TAR="$(mktemp /tmp/obs-build-XXXXXX.tar)"
box pull "$BOX" /home/user/out/build.tar "$TAR"
tar xf "$TAR" -C "$OUT" --strip-components=1
rm -f "$TAR"
echo "build is in $OUT ($(wslpath -w "$OUT" 2>/dev/null || echo "$OUT"))"
if [ "$DOWN" = 1 ]; then box down "$BOX"; fi
