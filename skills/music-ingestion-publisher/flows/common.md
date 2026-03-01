# Common Operations

Shared procedures for all ingestion flows.

## CRITICAL: Never Write Directly to LanceDB

The songs table uses Lance blob v2 encoding for `audio_data`
(`Struct<data: LargeBinary?, uri: Utf8?>`).
Direct writes (Python lancedb, arrow, manual RecordBatch, etc.) **WILL** corrupt
the table — the fragment's `audio_data` column encoding won't match blob v2
struct layout, causing `lance-encoding decoder.rs` errors on read.

**ALWAYS** use `sf-cli write-music` for ingestion.

Binary location: `./bin/sf-cli` (preferred) → `./target/release/sf-cli` → PATH.

## Verification (Post-Write)

```bash
sf-cli db --db-path /mnt/e/static-flow-data/lancedb-music \
  query-rows songs \
  --where "id='<song_id>'" \
  --columns id,title,artist,album,album_id,cover_image,format
```
Checklist:
- `title`, `artist`, `album` populated (not "Unknown")
- `album_id` set for Netease tracks
- `cover_image` is a valid `https://` URL (not http, not empty)
- `format` is `mp3` or `flac`

## Cover Image Update

**Recommended**: use `--cover-url` during ingestion (one-step):
```bash
sf-cli write-music \
  --db-path /mnt/e/static-flow-data/lancedb-music \
  --file /tmp/music/<file>.mp3 \
  --cover-url "https://..." \
  ... # other flags
```

**Fallback** (post-write update, if cover URL was not available at ingest time):
```bash
sf-cli db --db-path /mnt/e/static-flow-data/lancedb-music \
  update-rows songs \
  --where "id='<song_id>'" \
  --set "cover_image='<cover_url>'"
```
- Bilibili covers: API returns `http://` — **always convert to `https://`**
  (Bilibili CDN supports https; http causes mixed-content blocking).
- Netease covers: already `https://`.
- Accepts full URL or filename (frontend constructs `/api/images/<filename>`).

## Lyrics (Cross-Source)

Bilibili has no lyrics API. For Bilibili-sourced songs, fetch lyrics from
Netease if the original `netease_track_id` is known:
```bash
ncmdump-cli lyric <netease_track_id>
```
Split output at `--- Translation ---` into `.lrc` and `.tlyric.lrc` files.

## Vector Embedding Backfill

`write-music` auto-generates vectors at ingest time. Only run manually for
songs imported before vector support:
```bash
sf-cli embed-songs --db-path /mnt/e/static-flow-data/lancedb-music
sf-cli ensure-indexes --db-path /mnt/e/static-flow-data/lancedb
```

## Error Handling

| Error | Action |
|-------|--------|
| Netease 403/VIP/copyright | Switch to Bilibili Flow E; keep Netease for lyrics |
| Bilibili cookie expired | `ncmdump-cli bili-login` (QR, lasts ~6 months) |
| Netease cookie expired | `ncmdump-cli login <MUSIC_U>` |
| Network timeout | Retry up to 3 times, 5s delay |
| Duplicate ID | Skip unless user confirms overwrite |
| Cover URL missing | Fetch from source API. Never leave empty |
| Cover URL is http:// | Convert to https:// before writing |
| M4A disguised as MP3 | `file <path>` to detect, ffmpeg to convert |
| ffmpeg not found | Install before Bilibili downloads |

## Output Contract

Report after each ingestion:
- `id`, `title`, `artist`, `album`, `album_id`
- `cover_image` (confirm non-empty, https)
- `format` (mp3/flac), `duration_ms`
- `status`: success or error message
