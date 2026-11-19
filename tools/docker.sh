#!/bin/sh
# Run a build command in the Docker build environment (docker/Dockerfile), as the host's user so target/ stays yours.
#
#   tools/docker.sh                      # tools/mkbtf.sh tx: target/ferrite-tx.ffw, the user build
#   tools/docker.sh tools/mkbtf.sh reader
#   tools/docker.sh cargo test
#   tools/docker.sh bash                 # a shell in the build environment
#
# The image is built on first use and rebuilt when rust-toolchain.toml changes (compose's layer cache).
set -e
cd "$(dirname "$0")/.."
[ $# -eq 0 ] && set -- tools/mkbtf.sh tx
docker compose build -q build
exec docker compose run --rm --user "$(id -u):$(id -g)" build "$@"
