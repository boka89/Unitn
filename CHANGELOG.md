# Changelog

Older versions: see the [changelog on the website](https://unitn.bokors.net/changelog/).

## 0.7.2 — 2026-11-25

Fixes and a better search, from **@jo-el414**'s testing of 0.7.0/0.7.1 (#40–#42). Thanks!

- **Search suggestions** (#42): on the desktop, typing in the search box no longer jumps to the
  Songs page. A list opens under the box with matching artists, albums and songs: click an
  artist to see their albums, an album to open it, or a song to play it. "Show all songs" (or
  Enter) shows every matching song on the Songs page as before. The page you're on is still
  filtered while you type.
- **Artists page** (#41): while artist pictures load, the circles stay plain instead of
  flashing purple with the artist's initial, and the pictures fade in.
- **Statistics** (#40): every row of the top lists has the same full-width background; a thin
  bar along the bottom shows how its plays compare with the most played.

## 0.7.1 — 2026-11-25

One version for the desktop and Android: the Android app (still experimental) now has
everything up to 0.7.0, and the desktop gets the server library and the Plex improvements made
for Android.

### New

- **Your servers' music in the library**: the albums and songs of your Jellyfin, Plex and
  Navidrome servers are saved and appear in Albums, Artists and Songs next to your own music
  (a small label on the cover says which server). The search box finds them without asking the
  servers each time, and their covers load as you scroll.
- **Plex**: the Add server dialog lists the Plex servers it finds on your home network. With an
  address, Unitn first tries to connect without signing in (works when your Plex server allows
  your network). `*.plex.direct` addresses work even where the router blocks their DNS, an error
  names the server that can't be reached, and the browser sign-in keeps waiting through short
  network drops.
- **Android (experimental)**, now the same version as the desktop: sorting, the Artists grid,
  Statistics (Settings → Statistics), radio browsing by country, language, tag, format and
  bitrate, "Continue where I left off", reordering playlist songs (song menu → Move to top /
  bottom), the album's genre, the lower memory use of 0.6.6–0.6.7, and the album cover in the
  notification and on the lock screen.

### Not on Android

The mini player, drag and drop, keyboard shortcuts, window transparency, and saving lyrics into
files or songs (Android apps can't change music files; lyrics found online still show).

## 0.7.0 — 2026-11-25

Four new features requested by **@jo-el414** (#33–#36). Thanks!

### New

- **Continue where you left off** (#33): when Unitn opens, the queue, the song and where you
  were in it come back, ready to play. Settings → "Continue where I left off" turns it off.
- **Save lyrics found online** (#34): when lyrics come from LRCLIB, "Save as file" (a `.lrc` or
  `.txt` next to the song) and "Save in song" (the song's lyrics tag) buttons appear in the
  lyrics panel. Settings → "Save lyrics found online" can do it automatically: as a file, in the
  song, or both. Only for songs on your computer.
- **Browse radio** (#35): Country, Language, Tag, Format and Bitrate buttons open a searchable
  list from Radio Browser, sorted by how many stations each has. They combine with the genre
  chips, Favorites and the search box. They replace the Hungary chip.
- **Statistics** (#36): a new page in the sidebar with plays, listening time, top artist and
  genre, the top 10 songs, artists, albums and genres (last 30 days or all time), a chart of
  listening time per day, and your library's songs, albums, artists, genres and total length.
  A song counts once you've listened to 30 seconds of it (half, for songs under a minute);
  skipping ahead doesn't count as listening. Radio and podcasts aren't counted. The history
  starts with this version.

## 0.6.7 — 2026-11-24

More memory work from **@jo-el414**'s testing (#30). Thanks!

### Fixed

- **Memory no longer depends on the size of your library.** The Albums and Artists grids keep
  only the pictures on screen (plus a screen's worth above and below) in memory; scrolling
  loads and frees them, and leaving a page frees its grid. Before, every album cover in the
  library was in memory all the time, and visiting the Artists page added every artist.
- **Thumbnail folder**: album covers and artist photos, your own `artist.jpg` files included,
  are saved once as small 320 px copies in a `thumbs` folder in Unitn's data folder. The grids
  read those instead of decoding the full-size images every time.
- Only the last three full-size images (Now Playing cover, artist photo) are kept, instead of
  six.

Measured with a 4000-song, 200-album, 160-artist test library (Linux):

| | 0.6.6 | 0.6.7 |
|---|---|---|
| Playing on Now Playing with lyrics | 308 MB | 231 MB |
| Artists → artist → album → Now Playing | 379 MB | 261 MB |
| After 60 songs on shuffle | 452 MB | 305 MB |

The mini player doesn't leak: switching it on and off 30 times adds about 4 MB.

## 0.6.6 — 2026-11-24

From **@jo-el414**'s testing of 0.6.5 (#30). Thanks!

### New

- **Setting: "Ignore The, A and An when sorting"** (Settings, on by default). Turn it off to
  sort names exactly as written, so *The Beatles* comes under T.

### Fixed

- **Memory use**: 0.6.5 decoded a picture for every artist at startup, even when the Artists
  page was never opened, and kept every artist photo shown behind Now Playing in memory at
  full size. Now artist pictures are only in memory while the Artists page is shown, list
  thumbnails are smaller (320 px), and big photos are shrunk to screen size with only the last
  few kept. With a 4000-song test library, startup memory went from 420 MB (0.6.5) to 270 MB,
  lower than 0.6.1's 314 MB.
- CI checks memory on every build with a 4000-song test library, so a jump like this doesn't
  go unnoticed again.

## 0.6.5 — 2026-11-22

Everything from **@jo-el414**'s latest round of testing and ideas (#24–#28). Thanks!

### New

- **Sorting** on the Albums, Artists and Songs pages: a **Sort** button in the header opens the
  orders, the arrow next to it flips the direction, and the choice is remembered. (#25)
  - Albums: Artist (each artist's albums oldest to newest), Title, Year (newest first),
    Recently added.
  - Artists: Name, Most albums, Most songs, Recently added.
  - Songs: Artist, Title, Album, Year, Length, Genre, Recently added.
  - Names ignore a leading "The", "A" or "An" (The Beatles under B); albums and songs
    without a year or genre stay at the end.
  - Unitn now remembers when each song was added to the library. Songs already in it use
    their file's date.
- **Artists page redesign**: a grid of round artist pictures with the number of albums and
  songs. The picture is your own `artist.jpg`, else a downloaded photo, else one of the
  artist's album covers. Missing photos are downloaded slowly in the background (when album
  art downloads are on) and appear as they arrive. (#27)
- **Lyrics in the mini player**: the lyrics button shows a row with the line being sung. (#28)

### Fixed

- **Windows media overlay**: shows the name "Unitn" now. The installer's shortcuts carry an
  app ID that the app itself never claimed, so Windows couldn't tell which app was playing.
  (#24)
- **Mini player position**: the mini player comes back where you left it and at its width,
  also after restarting; if Unitn was closed as the mini player, it starts as one. The full
  window's position is remembered too. A window on a monitor that's gone is moved back onto
  the screen. (#24)
- **Albums page**: the album covers grow to fill each row, so there's no empty strip on the
  right. (#26)

## 0.6.1 — 2026-11-21

Two features asked for by **@jo-el414** (#19, #21). Thanks!

### New

- **Mini player**: the new button on the player bar (or **⌘/Ctrl+Shift+M**) shrinks Unitn to a
  small window with the cover, song, previous / play / next, like, seek bar and volume (mute
  and mouse wheel work there too). It stays on top of other windows; the pin button turns that
  off. The expand button, a double-click on the cover or the shortcut bring the full window back
  at its previous size. (#19)
- **Reorder playlist songs**: drag the handle at the start of a song to move it, like in the
  Queue. For long playlists the song menu has **Move to top** and **Move to bottom**. Works for
  every playlist, also with songs from servers and podcast episodes. (#21)

## 0.5.17 — 2026-11-21

More from **@jo-el414**: thanks for testing 0.5.16 and the new ideas (#14, #16, #19, #20).

### New

- **Genre on the album page**, after the year (from the songs' genre tags; for albums mixing
  genres, the two most common). Local music for now. (#20)
- **Artist photos folder**: downloaded artist photos are saved in an `artists` folder next to
  `covers` in Unitn's data folder, named after the artist (`Daft Punk.jpg`). Photos from
  earlier versions are moved there the next time the artist plays. (#16)
- **Windows media overlay** (volume keys / Bluetooth headset): shows the album cover and the
  name "Unitn". The cover also appears in the macOS Now Playing widget and Linux media
  controls. (#19)

### Fixed

- Windows: the vertical scrollbar no longer covers the right end of list rows (the highlight of
  the playing song, the stripes and the "…" buttons) in any list, also when it widens on hover.
  The 0.5.16 fix only handled a horizontal scrollbar. (#14)

## 0.5.16 — 2026-11-21

Thanks to **@jo-el414** for the bug reports and ideas (#14–#17).

### New

- **Mute button**: click the speaker next to the volume bar to mute, click again to get the
  previous volume back. Or press **M**. (#15)
- **Mouse-wheel volume**: scroll over the volume bar (or the speaker) to change the volume. (#15)
- **Lyrics subfolder**: lyrics can be kept in a `Lyrics` (or `lyrics`) subfolder of the album
  folder, named like the song files, so the album folder stays tidy. (#17)
- **`.txt` lyrics**: plain-text lyrics files work too, next to the song or in the subfolder.
  Files saved by older Windows apps (Windows-1252) or Notepad's "Unicode" (UTF-16) are read
  correctly. (#17)
- **Your own artist photos**: an `artist.jpg` (or `.jpeg`, `.png`, `.webp`) in the album folder
  or the artist folder above it is shown behind Now Playing instead of a downloaded photo, and
  nothing is downloaded for that artist. (#16)
- **Drag and drop** onto a playlist or the Queue in the sidebar: songs, albums, artists, podcast
  shows and episodes. Drop on the playlists "+" to make a new playlist from them. Podcast
  episodes can now be in playlists.
- **Like** button (heart) on the player bar, level with the play controls; greyed out when
  nothing likeable is playing.

### Fixed

- Windows: a horizontal scrollbar could appear over the last row of the Albums, album and
  Songs lists (seen at 125 % / 150 % display scaling). Lists now only scroll vertically, and
  the album grid leaves room for the vertical scrollbar. (#14)
- Lyrics: lines scrolling past the top of the lyrics panel no longer run over its border. (#14)
- Faster startup with big libraries: album covers are shrunk and loaded in the background
  instead of holding up the window, and a "Syncing library…" pill shows while the folder scan
  runs.
- A library scan no longer blocks likes and playlist changes ("database is locked").
- The sidebar no longer has a separate "Now Playing" entry.
