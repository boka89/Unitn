# Unitn player

**Unitn player** (say "unitone player") is a free, open-source music player for
**Windows, macOS and Linux** (and, experimentally, **Android**), written in Rust. Think "VLC for music", with a
modern UI: your local music library, your **Jellyfin, Plex and Navidrome** servers,
thousands of internet radio stations and your **podcasts**, in one fast app with no ads,
no account and no tracking.

**Website and downloads: [unitn.bokors.net](https://unitn.bokors.net)**

> Status: v0.7.1 — young but usable. Feedback and contributions welcome.

![Unitn playing an album, dark theme](docs/screenshots/dark-playing.png)

| Now Playing with the Winamp-style equalizer | Your music server |
|---|---|
| ![Equalizer](docs/screenshots/dark-nowplaying-eq-rock.png) | ![Server albums](docs/screenshots/dark-server.png) |
| **Internet radio** | **Light theme** |
| ![Radio](docs/screenshots/dark-radio-playing.png) | ![Light theme](docs/screenshots/light-nowplaying-eq-rock.png) |
| **Fix album info** (MusicBrainz) | **Full-screen visualizer** |
| ![Fix album info](docs/screenshots/dark-album-fix.png) | ![Visualizer](docs/screenshots/dark-viz-4.png) |
| **Synced lyrics** | **Podcasts** |
| ![Lyrics](docs/screenshots/dark-lyrics.png) | ![Podcasts](docs/screenshots/dark-podcast-show.png) |

## Features

- **Local library**: scans your music folders (MP3, FLAC, AAC/M4A, ALAC, OGG Vorbis, WAV, AIFF),
  reading tags and embedded or folder cover art
- **Albums, Artists and Songs** views (album pages show year and genre) with instant search, Apple-style striped track lists,
  Comfy and Cozy list density, and a play marker on the current song. Artists are a grid of
  photos (your own `artist.jpg`, a downloaded photo or an album cover)
- **Sorting** on every library page, remembered: Albums by artist, title, year or recently
  added; Artists by name, most albums, most songs or recently added; Songs by artist, title,
  album, year, length, genre or recently added. Names ignore a leading "The", "A" or "An"
  (can be turned off in Settings)
- **Album art**: missing covers are downloaded from the free MusicBrainz / Cover Art Archive,
  and artist photos appear behind Now Playing: your own `artist.jpg` (`.png`, `.webp`) from the
  artist or album folder if you have one, otherwise a photo from Wikimedia Commons
- **Fix album info**: search MusicBrainz by hand, pick the right album and cover (or use your
  own image) and correct artist/album names. Your music files are never modified
- **Music servers**: Jellyfin, Plex, Navidrome and other OpenSubsonic servers. Their albums and
  songs appear in your Albums, Artists and Songs views next to your own music (labelled with the
  server), or browse a server on its own page; they play like local music, seeking and gapless
  included. Search finds songs on your servers too, and server songs can go into playlists.
  See [Music servers](#music-servers)
- **Podcasts**: search Apple's public podcast directory or paste any RSS link, subscribe, and
  play episodes. Unitn remembers where you stopped in each episode and shows what's new
- **Lyrics** on Now Playing: synced lyrics scroll along with the song and highlight the current
  line. They come from a `.lrc` or `.txt` file named like the song (next to it, or in a `Lyrics`
  subfolder of the album folder), from the song's own tags, or online from the free
  [LRCLIB](https://lrclib.net) (can be turned off). Lyrics found online can be saved next to the
  song (`.lrc`/`.txt`) or into its tags, with a button or automatically (Settings)
- **Internet radio** with ~50,000 stations from the open [Radio Browser](https://www.radio-browser.info),
  Popular / Favorites, genre chips, browse by country, language, tag, format and bitrate, and your own stream URLs
- **Playlists**: create, rename, reorder songs (drag the handle, or "Move to top / bottom" in
  the song menu), import/export M3U, and save the queue as a playlist; local songs and songs from
  your servers can be mixed
- **Queue** with drag-to-reorder, shuffle, repeat all / repeat one. Unitn starts where you left
  off: the queue, the song and the position come back when you open it (can be turned off)
- **Artist pages** with the artist's photo, Play / Shuffle, their albums and a biography from
  [TheAudioDB](https://www.theaudiodb.com) in your computer's language (several languages; can
  be turned off). Missing artist photos are downloaded from TheAudioDB, then Wikimedia Commons
- **Statistics**: plays and listening time for the last 30 days or all time, top songs,
  artists, albums and genres, listening time per day and your library's totals. A song counts
  once you've listened to 30 seconds of it (half of a song shorter than a minute)
- **Gapless playback** and **crossfade** (2–12 s; optionally keep songs of the same album gapless), and
  whole-file preloading so music from network shares doesn't stutter
- **Winamp-style 10-band equalizer** with preamp, presets and a live response curve
- **Visualizer** with five styles (spectrum with falling bars, mirror, wave, oscilloscope, radial),
  tuned for both themes; the ‹ › buttons (or ← →) switch styles and a button goes full screen. The
  buttons appear when you move the mouse and fade out when it rests
- **Drag & drop** files, folders and M3U playlists onto the window
- **Volume** with a mute button (click the speaker, or press M) and mouse-wheel control over the
  volume bar
- **Mini player**: a small window with the cover, song, play controls, like, seek bar, volume and
  optionally a line of synced lyrics, that stays on top of other windows (can be unpinned) and
  remembers where you put it. Open it with the button on the player
  bar or ⌘/Ctrl+Shift+M; the expand button or a double-click on the cover brings the full window back
- **Keyboard shortcuts** (Space, ←/→ seek, ⌘/Ctrl+←/→ track, ↑/↓ volume, M mute, ⌘/Ctrl+F search,
  ⌘/Ctrl+Shift+M mini player …)
- **Dark and light themes**, or follow the system setting; adjustable window transparency and,
  on macOS, a frosted-glass (blurred) window background
- **Media keys and OS integration**: macOS Now Playing, Windows media overlay, Linux MPRIS;
  optionally opens when you log in and keeps the computer awake while music plays
- Long titles scroll on the player bar and Now Playing; songs play with one click or, if you
  prefer, a double click
- Native Rust audio engine (symphonia + cpal), with no web view and no Electron

## Download

Get the latest installers from the **[Releases page](https://github.com/boka89/Unitn/releases/latest)**:

- **Windows**: `…_x64-setup.exe` installer, or the portable `.zip`
- **macOS**: `…_aarch64.dmg` (Apple Silicon) or `…_x64.dmg` (Intel)
- **Linux**: `.deb` (Debian/Ubuntu), `.AppImage` (any distro) or portable `.tar.gz`
- **Android** (experimental, 8.0 or newer): `…_android.apk`. Open it on the phone and allow
  installing apps from your browser or file manager. It has a phone layout with tabs, plays on in
  the background with lock-screen and headset controls, and finds the music in the phone's Music
  and Download folders. It has been tested in the Android emulator only so far; please report
  what you find on a real phone in [Issues](https://github.com/boka89/Unitn/issues).

The builds are not signed with paid certificates yet. On macOS, open the app the first time with
right-click → Open (or System Settings → Privacy & Security → "Open Anyway").

## Music servers

Unitn plays the music on your own **Jellyfin**, **Plex** or **Navidrome** server (or any other
OpenSubsonic / Subsonic server, such as Gonic or Airsonic). Open
**Settings → Music servers → Add server**:

| | Jellyfin | Plex | Navidrome / Subsonic |
|---|---|---|---|
| Address | `192.168.1.10` (port 8096 is assumed) or the full URL, e.g. `https://music.example.com` | optional: pick a server Unitn found on your network, leave it empty and Unitn finds your server after you sign in, or type `192.168.1.10` (port 32400 is assumed) or the full URL | `192.168.1.10` (port 4533 is assumed) or the full URL |
| Sign in | your Jellyfin user name and password | if your Plex server allows your network without signing in, Unitn connects right away; otherwise click **Sign in with Plex**: plex.tv opens in your browser, you sign in there (two-factor sign-in works), and Unitn picks up the access token. If no browser opens, go to [plex.tv/link](https://plex.tv/link) and enter the code Unitn shows | your user name and password |

The server then appears under **SERVERS** in the sidebar. Its albums open in the normal album
view; play, shuffle, queue and "play next" work as for local music. The search box at the top
also finds songs on all your servers (they are listed below your own songs), and any server
song can be added to a playlist, next to your local music.

Good to know:

- Songs are streamed as the **original files** (no transcoding) and loaded into memory in the
  background, so seeking is instant and albums play gaplessly. Formats Unitn can't decode
  (e.g. Opus, WMA) won't play yet.
- Unitn never sees your Plex password, and only uses your Jellyfin password to sign in. It stores
  the **access token** the server gives it (in `settings.json`, see below); for Subsonic servers a
  salted token (`md5(password + salt)`) instead of the password. Remove the server in
  Settings to forget it; you can also revoke the device under Jellyfin's Dashboard → Devices or
  Plex's Settings → Authorized Devices.
- HTTPS works with properly signed certificates; for self-signed certificates use the server's
  `http://` LAN address.

## Lyrics and artist photos in your music folders

Unitn picks up files you keep with your music; nothing has to be imported:

```
Music/
  Artist/
    artist.jpg            ← artist photo behind Now Playing (.jpg, .jpeg, .png or .webp)
    Album/
      cover.jpg           ← album cover (also folder.jpg, front.jpg …, or embedded in the files)
      01 Song.flac
      01 Song.lrc         ← lyrics next to the song …
      Lyrics/             ← … or all of the album's lyrics in a "Lyrics" (or "lyrics") subfolder
        02 Other song.lrc
        03 Third song.txt
```

- A lyrics file has the same name as the song file. `.lrc` (synced, the current line is
  highlighted) is used before `.txt` (plain text), and a file next to the song before one in
  the `Lyrics` folder. UTF-8, UTF-16 and older Windows-1252 text files all work.
- `artist.jpg` may be in the album folder or the artist folder above it. When it's there, Unitn
  uses it and doesn't download a photo.

## Making a release

Releases are fully automated by [`.github/workflows/release.yml`](.github/workflows/release.yml):

1. Bump `version` in the root `Cargo.toml` (and in `Cargo.lock`), and commit.
2. On GitHub open **Actions → Release → Run workflow** and enter the version, e.g. `0.4.0`
   (or push a tag `v0.4.0`).
3. GitHub Actions builds every installer and publishes the release with notes.

Every push is built and tested on all three systems by
[`build.yml`](.github/workflows/build.yml): unit tests, a playback smoke test on macOS
(files, radio, gapless), a radio stream check, a Windows startup test, real **Jellyfin and
Plex servers in Docker** that Unitn signs in to and streams from, and the screenshots
above (published to the `ci-screenshots` branch).

To test the packaging without publishing, run the **Release** workflow manually from the Actions tab.

## Building from source

Install Rust (<https://rustup.rs>), then:

```sh
git clone https://github.com/boka89/Unitn
cd Unitn
cargo run --release
```

On **Linux** you also need the ALSA and font/keyboard development packages:

```sh
# Debian / Ubuntu
sudo apt install libasound2-dev libfontconfig1-dev libxkbcommon-dev
# Fedora
sudo dnf install alsa-lib-devel fontconfig-devel libxkbcommon-devel
```

Windows and macOS need no extra packages.

## Project layout

```
crates/
  unitn-core/   audio engine, library index (SQLite), radio client (no UI code)
    src/audio/       decoding (symphonia), output (cpal), HTTP/ICY radio streams
    src/library/     folder scanner (lofty tags) + SQLite database
    src/radio.rs     Radio Browser API client
    src/servers.rs   Jellyfin and Plex clients
    src/artwork.rs   album art and artist photos (MusicBrainz, Cover Art Archive, Wikimedia)
  unitn/        the desktop app (Slint UI)
    ui/              .slint files: theme, widgets, pages, main window, icons
    src/app.rs       app state: queue, playback, library and radio wiring
    src/media.rs     media keys / OS now-playing integration (souvlaki)
    src/glass.rs     macOS frosted-glass window background
```

The UI is written in [Slint](https://slint.dev). To tweak the design with live
preview, install the Slint VS Code extension or `slint-viewer` and open
`crates/unitn/ui/app.slint`.

## Where data is stored

| OS      | Library database, covers, artist photos           | Settings                                 |
|---------|---------------------------------------------------|------------------------------------------|
| Linux   | `~/.local/share/unitn/`                           | `~/.config/unitn/settings.json`          |
| macOS   | `~/Library/Application Support/net.bokors.Unitn/` | same folder                              |
| Windows | `%APPDATA%\bokors\Unitn\data\`                   | `%APPDATA%\bokors\Unitn\config\`        |

Inside the data folder: `library.sqlite3` (the library), `covers/` (album art cache),
`artists/` (downloaded artist photos, named after the artist, e.g. `Daft Punk.jpg`) and
`thumbs/` (small copies of covers and artist photos for the grids; safe to delete).
Your own `artist.jpg` files stay in your music folders and are not copied.

On first start, your system Music folder is added automatically.

## Roadmap

- High-quality resampler, ReplayGain
- Podcast downloads for offline listening
- HLS radio streams, full-quality HE-AAC, Opus

## License

GPL-3.0-or-later. Icons are based on Google's Material Icons (Apache-2.0).
