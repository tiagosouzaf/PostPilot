#!/usr/bin/env bash
# Roda cada protótipo N vezes sob Xvfb e imprime as métricas.
# Uso: ./bench-linux.sh [repetições] [segundos em repouso]
set -euo pipefail
N=${1:-3}
IDLE=${2:-10}
cd "$(dirname "$0")"
run() { # nome comando env...
  local name=$1 bin=$2; shift 2
  for i in $(seq "$N"); do
    out=$(env "$@" SPIKE_BENCH=1 SPIKE_IDLE_SECS="$IDLE" timeout 180 \
      xvfb-run -a -s "-screen 0 1600x1000x24" "$bin" 2>/dev/null | grep '=' | tr '\n' ' ')
    echo "$name run=$i $out"
  done
}
[ -x egui/target/release/spike-egui ] && {
  run egui-wgpu egui/target/release/spike-egui SPIKE_RENDERER=wgpu
  run egui-glow egui/target/release/spike-egui SPIKE_RENDERER=glow
}
[ -x slint/target/release/spike-slint ] && run slint-femtovg slint/target/release/spike-slint SLINT_BACKEND=winit-femtovg
[ -x gpui/target/release/spike-gpui ] && run gpui gpui/target/release/spike-gpui
true
