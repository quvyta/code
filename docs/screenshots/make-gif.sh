#!/bin/sh
# Draws the README's moving picture again: qcode.gif and qcode.mp4 in this folder.
#
# The `readme_gif` test in tests/readme_shots.rs records the application with the framework's
# `qshots::Reel`, so the picture shows only the demo workspace and is the same on every machine;
# the frames go to target/readme-gif/ and the test has ffmpeg join them, each held as long as the
# script says. Needs ffmpeg with libx264.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

cd "$root"
# Release, because drawing some hundreds of large PNGs is slow without optimisation.
cargo test --release --test readme_shots readme_gif -- --ignored --nocapture

ls -l "$here/qcode.gif" "$here/qcode.mp4"
