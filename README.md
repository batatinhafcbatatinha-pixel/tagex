# Tagex

A fast, native file explorer for Windows that has **no folders — only tags**.

Point Tagex at any number of locations (local folders, drives or SMB network shares).
It indexes every file into one flat library, and from then on you find things by
combining tags, file types and search — never by remembering which folder something is in.

Successor of [gerenciador](https://github.com/lucashartmann/gerenciador), rewritten in Rust.

## Features

- **Tags instead of folders** — click tags to combine them (AND), Alt+click to exclude,
  "Untagged" to find what still needs sorting.
- **Fast tagging** — press `1`–`9` to toggle a tag on the selection, drag files onto a tag,
  or type a tag name in the side panel (Enter creates it if it doesn't exist).
- **Search** — words match file names, `#word` matches tag names (`beach #2024 #fav`).
- **SMB / network shares** — add `\server\share` with optional credentials (or your
  Windows login), browse a server's shares, reconnect offline locations. The index stays
  browsable while a share is offline.
- **Native thumbnails and icons** from the Windows shell (photos, videos, PDFs, Office…),
  cached on disk so network folders load instantly the second time.
- **Tags survive moves and renames** — when a tagged file disappears and an identical file
  (same name, size and date) shows up elsewhere, the tags follow it.
- **English and Portuguese** — follows the Windows display language (Portuguese if Windows
  is in Portuguese, English otherwise); change it any time in the ⚙ settings menu.
- Grid and list views, sorting, multi-select (Ctrl/Shift), rename, move to Recycle Bin,
  show in Explorer, copy path.

## Why Rust

Same speed as C/C++ (no garbage collector, no runtime), but memory-safe. The UI is
[egui](https://github.com/emilk/egui) rendered on the GPU, so scrolling through hundreds of
thousands of files stays smooth: only visible rows are laid out, and the whole library
lives in memory with tags as sorted id lists.

The indexer walks directories with a pool of threads (24 for network shares) and takes
size/date straight from `FindNextFileW`, so it never opens individual files.

## Build

Requires Rust (stable, MSVC toolchain) on Windows 10/11.

```sh
cargo build --release
target/release/tagex.exe
```

## Shortcuts

| Key | Action |
| --- | --- |
| `1`–`9` | Toggle the n-th tag on the selection |
| `Ctrl+F` | Search |
| `Ctrl+A` / `Esc` | Select all / clear selection |
| `Enter` | Open |
| `F2` | Rename |
| `Del` | Move to Recycle Bin |
| Alt+click a tag | Hide files with that tag |

## Data

The library lives in `%APPDATA%\Tagex\data\library.db` (SQLite) and thumbnails in
`%LOCALAPPDATA%\Tagex\cache\thumbs`. Set `TAGEX_DATA` to use a different library folder.
Removing a location only forgets its files; nothing is ever deleted from disk except
through "Move to Recycle Bin".

## Layout

| File | What |
| --- | --- |
| `src/app.rs` | Window, filters, selection, dialogs |
| `src/db.rs` | SQLite schema and queries |
| `src/scanner.rs` | Parallel indexer |
| `src/thumbs.rs` | Shell thumbnails/icons + disk cache |
| `src/smb.rs` | SMB connect and share listing (WNet / NetApi) |
| `src/i18n.rs` | English/Portuguese strings and language detection |
| `src/model.rs`, `src/theme.rs` | Data types, look and feel |
