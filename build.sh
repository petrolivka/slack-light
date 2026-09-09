#!/usr/bin/env bash
# Build in a cgroup of its own, so a runaway compile is killed instead of the
# terminal it was started from. See .cargo/config.toml for why.
#
#   ./build.sh                     # cargo build --release --bins
#   ./build.sh test --release      # anything else cargo takes
set -euo pipefail
args=("$@")
[ ${#args[@]} -eq 0 ] && args=(build --release --bins)

# `cargo test` in a virtual-manifest-plus-root-package workspace tests the root
# package only, which here is a thin binary with no tests at all: "0 passed"
# reads as success and covers nothing. Every crate, unless the caller picked one.
case "${args[0]}" in
    test|clippy)
        if [[ ! " ${args[*]} " =~ " --workspace " && ! " ${args[*]} " =~ " -p " ]]; then
            args=("${args[0]}" --workspace "${args[@]:1}")
        fi
        ;;
esac

if command -v systemd-run >/dev/null 2>&1; then
    exec systemd-run --user --scope --quiet --collect \
        -p MemoryMax=8G -p MemorySwapMax=4G -p MemoryPressureWatch=off \
        -- cargo "${args[@]}"
fi
exec cargo "${args[@]}"
