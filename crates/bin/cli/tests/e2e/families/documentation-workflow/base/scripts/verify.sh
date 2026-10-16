#!/bin/sh
set -eu

usage() {
    echo "usage: scripts/verify.sh [--quick | --complete]"
    echo "  --quick     check that the crate compiles"
    echo "  --complete  check formatting, then run every test"
}

case "${1:-}" in
    --quick)
        cargo check --offline
        ;;
    --complete)
        cargo fmt --check
        cargo test --offline
        ;;
    --help | -h)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
