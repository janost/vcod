#!/usr/bin/env bash
# Builds docs/screenshots from full-size PNGs: a full-res JPEG and a thumbnail
# per shot, and the README collage (two shots per row, in argument order).
# Each PNG is named <game>-<rest>.png; <game> (cod1, uo, cod2, cod4) picks the
# collage label. Needs ImageMagick 7 and jpegoptim.
#
#   tools/screenshots.sh shots/cod1-mp_pavlov.png shots/uo-mp_arnhem.png ...
set -euo pipefail

out="$(dirname "$0")/../docs/screenshots"
mkdir -p "$out"

# Collage geometry: 2 columns, 21:9 tiles (3840x1600 is 2.4:1).
width=1920 gap=10 bg='#0d1117'
tile_w=$(( (width - 3 * gap) / 2 ))
tile_h=$(( tile_w * 10 / 24 ))

label() {
  case "$1" in
    cod1) echo "Call of Duty" ;;
    uo) echo "United Offensive" ;;
    cod2) echo "Call of Duty 2" ;;
    cod4) echo "Call of Duty 4" ;;
    *) echo "unknown game prefix: $1" >&2; exit 1 ;;
  esac
}

tiles=()
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for png in "$@"; do
  name=$(basename "$png" .png)
  game=${name%%-*}
  # Full res: 4:4:4 keeps the coloured HUD and overlay text sharp.
  magick "$png" -strip -sampling-factor 1x1 -quality 85 "$out/$name.jpg"
  magick "$png" -strip -resize 1600x -sampling-factor 2x2 -quality 82 "$out/$name-thumb.jpg"
  magick "$png" -resize "${tile_w}x${tile_h}^" -gravity center -extent "${tile_w}x${tile_h}" \
    \( -background '#000000b8' -fill white -font DejaVu-Sans-Bold -pointsize 17 \
       label:"$(label "$game")" -bordercolor '#000000b8' -border 10x5 \) \
    -gravity northwest -geometry +12+12 -composite \
    "$tmp/tile-${#tiles[@]}.png"
  tiles+=("$tmp/tile-${#tiles[@]}.png")
done

magick montage "${tiles[@]}" -tile 2x -geometry "${tile_w}x${tile_h}+$(( gap / 2 ))+$(( gap / 2 ))" \
  -background "$bg" "$tmp/montage.png"
magick "$tmp/montage.png" -bordercolor "$bg" -border $(( gap / 2 )) \
  -strip -sampling-factor 2x2 -quality 84 "$out/collage.jpg"

jpegoptim -q --strip-all --all-progressive "$out"/*.jpg
