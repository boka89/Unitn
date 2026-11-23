#!/usr/bin/env bash
# Memory check: runs Unitn on a library the size of a real user's (160
# artists, 200 albums, 4000 MP3s, covers and big artist.jpg photos) and
# reports its memory (resident set size) at three points:
#   1. library scanned, Albums page
#   2. Artists page opened (artist pictures loaded)
#   3. a song playing on Now Playing with the lyrics panel open
# The numbers go to the job summary. Above LIMIT_MB a warning is shown, so a
# release that suddenly needs much more memory doesn't go unnoticed.
set -euo pipefail

LIMIT_MB=${LIMIT_MB:-400}
BIN="$PWD/target/release/unitn"
export HOME="$(mktemp -d)"
MUSIC="$HOME/Music"
OUT="$PWD/memory"
mkdir -p "$MUSIC" "$OUT"

# --- library --------------------------------------------------------------
echo "Generating the test library…"
ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=440:duration=4" -c:a libmp3lame -q:a 9 /tmp/base.mp3
ARTISTS=160 ALBUMS=200 SONGS=20
jobs_file=$(mktemp)
for a in $(seq 1 $ARTISTS); do
  artist="Artist $(printf %03d $a)"
  mkdir -p "$MUSIC/$artist"
  # A big artist photo, like the high-quality ones people keep.
  ffmpeg -loglevel error -y -f lavfi -i "testsrc2=s=1600x1600" -frames:v 1 -q:v 4 "$MUSIC/$artist/artist.jpg"
done
for b in $(seq 1 $ALBUMS); do
  a=$(( (b - 1) % ARTISTS + 1 ))
  artist="Artist $(printf %03d $a)"
  album="Album $(printf %03d $b)"
  dir="$MUSIC/$artist/$album"
  mkdir -p "$dir"
  ffmpeg -loglevel error -y -f lavfi -i "color=c=0x$(printf %06x $((b * 83311 % 16777215))):s=1000x1000" -frames:v 1 "$dir/cover.jpg"
  for s in $(seq 1 $SONGS); do
    printf '%s\t%s\t%s\t%s\n' "$artist" "$album" "$s" "$dir/$(printf %02d $s) Song $s.mp3" >> "$jobs_file"
  done
done
# Tag copies of one short MP3 (fast: no re-encoding), four at a time.
tag() {
  IFS=$'\t' read -r artist album n file <<< "$1"
  ffmpeg -loglevel error -y -i /tmp/base.mp3 -c copy -id3v2_version 3 \
    -metadata title="Song $n" -metadata artist="$artist" -metadata album="$album" \
    -metadata track="$n" -metadata date=2020 -metadata genre=Rock "$file"
}
export -f tag
tr '\n' '\0' < "$jobs_file" | xargs -0 -P 4 -I{} bash -c 'tag "$1"' _ {}
# Synced lyrics for the first song (the lyrics panel shows them).
printf '[00:00.50]First line\n[00:02.00]Second line\n' > "$MUSIC/Artist 001/Album 001/01 Song 1.lrc"
echo "$(find "$MUSIC" -name '*.mp3' | wc -l) songs"

mkdir -p "$HOME/.config/unitn"
echo "XDG_MUSIC_DIR=\"$MUSIC\"" > "$HOME/.config/user-dirs.dirs"
# No network lookups: the numbers should only depend on the app.
echo '{"theme": 2, "volume": 0.0, "glass": false, "download_art": false, "lyrics_online": false, "lyrics_open": true, "startup_sound": false}' \
  > "$HOME/.config/unitn/settings.json"

# --- run --------------------------------------------------------------------
export DISPLAY=:98
Xvfb :98 -screen 0 1280x820x24 &
XVFB=$!
sleep 2
"$BIN" > "$OUT/log.txt" 2>&1 &
PID=$!
rss_mb() { awk '/VmRSS/ { printf "%d", $2 / 1024 }' "/proc/$PID/status"; }
click() { xdotool mousemove "$1" "$2" click 1; sleep "${3:-2}"; }

DB="$HOME/.local/share/unitn/library.sqlite3"
n=0
for i in $(seq 1 300); do
  n=$(sqlite3 "$DB" "SELECT COUNT(*) FROM tracks" 2>/dev/null || echo 0)
  [ "$n" -ge $((ALBUMS * SONGS)) ] && break
  [ $((i % 15)) -eq 0 ] && echo "  ${i}s: $n songs in the library"
  sleep 1
done
echo "scanned: $n songs after ${i}s"
if [ "$n" -lt $((ALBUMS * SONGS)) ]; then
  echo "::error title=Memory check::The test library was not scanned ($n of $((ALBUMS * SONGS)) songs); no numbers measured"
  echo "--- app log"; tail -n 60 "$OUT/log.txt" || true
  ls -la "$HOME/.local/share/unitn" || true
  kill $PID $XVFB 2>/dev/null || true
  exit 1
fi
sleep 15   # covers decoded in the background
WID=$(xdotool search --onlyvisible --name Unitn | head -1)
xdotool windowmove "$WID" 0 0 windowsize "$WID" 1280 820
sleep 3
M1=$(rss_mb)
import -window root "$OUT/1-albums.png"

click 120 160 20          # Artists page: its pictures load now
M2=$(rss_mb)
import -window root "$OUT/2-artists.png"

click 120 117 3           # Albums
click 341 160 3           # first album
click 524 257 4           # Play
click 45 776 15           # Now Playing (lyrics panel is open)
M3=$(rss_mb)
import -window root "$OUT/3-nowplaying.png"

kill $PID 2>/dev/null || true
kill $XVFB 2>/dev/null || true
echo "--- app log (last lines)"; tail -n 25 "$OUT/log.txt" || true

echo "RESULT memory: albums ${M1} MB, artists ${M2} MB, now playing ${M3} MB"
{
  echo "### Memory (4000 songs, 200 albums, 160 artists)"
  echo
  echo "| Step | Memory (RSS) |"
  echo "|---|---|"
  echo "| Library scanned, Albums | ${M1} MB |"
  echo "| Artists page opened | ${M2} MB |"
  echo "| Playing, Now Playing with lyrics | ${M3} MB |"
  echo
  echo "Warning limit: ${LIMIT_MB} MB."
} >> "${GITHUB_STEP_SUMMARY:-/dev/stdout}"
max=$(printf '%s\n' "$M1" "$M2" "$M3" | sort -n | tail -1)
if [ "$max" -gt "$LIMIT_MB" ]; then
  echo "::warning title=Memory::Unitn used ${max} MB with a 4000-song library (limit ${LIMIT_MB} MB)"
fi
