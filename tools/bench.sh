#!/usr/bin/env bash
# Measure the society's three latency-critical paths on this machine.
#
#   tools/bench.sh              # measure and write baseline/perf_baseline.json
#   tools/bench.sh --check      # measure and compare against the recorded budget
#
# No dependency beyond a Rust toolchain: the harness is a workspace crate
# (tools/bench) written on std alone.
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --release --quiet -p society-bench --bin society-bench -- "$@"
