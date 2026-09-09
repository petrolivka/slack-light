#!/usr/bin/env bash
# Build in a cgroup of its own, so a runaway compile is killed instead of the
# terminal it was started from. See .cargo/config.toml for why.
#
#   ./build.sh                     # cargo build --release --bins
#   ./build.sh test --release      # anything else cargo takes
set -euo pipefail
args=("$@")
[ ${#args[@]} -eq 0 ] && args=(build --release --bins)

if command -v systemd-run >/dev/null 2>&1; then
    exec systemd-run --user --scope --quiet --collect \
        -p MemoryMax=8G -p MemorySwapMax=4G -p MemoryPressureWatch=off \
        -- cargo "${args[@]}"
fi
exec cargo "${args[@]}"
