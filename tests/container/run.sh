#!/usr/bin/env bash
# Run novarch's system-changing tests in a throwaway Arch container. Nothing touches the
# host's packages: the container is deleted afterwards (--rm).
# Usage: tests/container/run.sh [chaotic|fallback|all]   (needs docker; network access)
set -euo pipefail
cd "$(dirname "$0")/../.."
mode="${1:-all}"
cargo build --release --quiet
results="$PWD/target/container-results"
mkdir -p "$results"

run_mode() {
    echo "=== mode: $1 (logs: $results/$1)"
    rm -rf "${results:?}/$1" && mkdir -p "$results/$1"
    docker run --rm \
        -v "$PWD/target/release/novarch:/usr/local/bin/novarch:ro" \
        -v "$PWD/tests/container:/tests:ro" \
        -v "$results/$1:/results" \
        archlinux:latest bash /tests/setup.sh "$1"
}

status=0
case "$mode" in
    all) run_mode chaotic || status=1; run_mode fallback || status=1 ;;
    chaotic | fallback) run_mode "$mode" || status=1 ;;
    *) echo "unknown mode $mode" >&2; exit 2 ;;
esac
exit $status
