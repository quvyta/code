#!/bin/sh
# Draws the README's moving picture again: qcode.gif and qcode.mp4 in this folder.
#
# The frames come from the test harness (the `readme_gif` test in tests/readme_shots.rs), so the
# picture shows only the demo workspace and is the same on every machine; ffmpeg joins them, each
# held as long as the test says. Needs ffmpeg with libx264.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
frames="$root/target/readme-gif"

cd "$root"
# Release, because drawing some sixty large PNGs is slow without optimisation.
cargo test --release --test readme_shots readme_gif -- --ignored

list="$frames/frames.txt"
palette="$frames/palette.png"
# The length the frames add up to; the frame rate filter would hold the last frame longer.
length=$(awk '$1 == "duration" { total += $2 } END { print total }' "$list")

ffmpeg -loglevel error -y -f concat -i "$list" -t "$length" \
    -vf "fps=25,scale=1280:-2:flags=lanczos,format=yuv420p" \
    -c:v libx264 -preset veryslow -tune animation -crf 20 -movflags +faststart -an \
    "$here/qcode.mp4"

ffmpeg -loglevel error -y -f concat -i "$list" \
    -vf "scale=1280:-1:flags=lanczos,palettegen=stats_mode=diff" \
    "$palette"
ffmpeg -loglevel error -y -f concat -i "$list" -i "$palette" \
    -lavfi "scale=1280:-1:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle" \
    -fps_mode passthrough -loop 0 \
    "$here/qcode.gif"

ls -l "$here/qcode.gif" "$here/qcode.mp4"
