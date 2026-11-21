# Changelog

Older versions: see the [changelog on the website](https://unitn.bokors.net/changelog/).

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
