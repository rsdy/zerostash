#!/bin/bash
set -e -u -o pipefail
cd $GITHUB_WORKSPACE
cargo +nightly build --release --target=x86_64-unknown-linux-musl
