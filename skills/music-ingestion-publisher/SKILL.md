---
name: music-ingestion-publisher
description: >-
  Ingest music into StaticFlow Music DB via ncmdump-rs CLI and sf-cli:
  search/download from Netease Cloud Music, decrypt local NCM files,
  extract metadata/lyrics/cover art, and publish to lancedb-music.
---

# Music Ingestion Publisher

Use this skill to ingest music files into the StaticFlow Music LanceDB.

## When To Use
1. Import music from Netease Cloud Music (search, download, ingest).
2. Decrypt local `.ncm` files and ingest the decoded audio.
3. Directly ingest local `mp3`/`flac` files with metadata.

## Preconditions
1. Resolve ncmdump CLI binary (actual name is `ncmdump-cli`):
   - `./tools/ncmdump-rs/target/release/ncmdump-cli`
   - `ncmdump-cli` from `PATH`
   - Build if needed: `cargo build -p ncmdump-cli --release`
     (run from `./tools/ncmdump-rs/`)
2. Resolve sf-cli in this order:
   - `./bin/sf-cli`
   - `./target/release/sf-cli`
   - `./target/debug/sf-cli`
   - `sf-cli` from `PATH`
   - Build if needed: `cargo build -p sf-cli --release`
3. Verify music DB path exists or will be auto-created:
   - Default: `/mnt/e/static-flow-data/lancedb-music`
4. For Netease online flows, verify login status:
   - `ncmdump-cli me` (shows current user if logged in)
   - If expired: `ncmdump-cli login qr` or `ncmdump-cli login phone`

## Hard Rules
- Never delete source audio files.
- Never overwrite existing song records unless user passes `--force` or
  explicitly confirms.
- **Cover image is MANDATORY** for Netease tracks. Every song must have
  `cover_image` populated with the album cover URL. Do NOT skip this step.
- **Album metadata is MANDATORY**. Always record `--album` and `--album-id`
  when available. Songs from the same album share the same `album_id`.
- All ingested songs must have `searchable_text` populated (auto-generated
  by sf-cli from title + artist + album + lyrics plain text).
- Verify the record exists after write, including `cover_image` field.

## Workflow

### Flow A: Netease Online Search & Download

1. **Search**: `ncmdump-cli search "<keyword>" --type track --limit 10`
   - Present results to user for selection.

2. **Get track info**: `ncmdump-cli info <track_id>`
   - Extract: artist, album name, album ID, duration.
   - Note: `info` command does NOT print cover URL. Cover URL must be
     fetched separately (see step 5).

3. **Download audio**:
   ```bash
   ncmdump-cli download <track_id> --quality exhigh \
     --output /tmp/music/<track_id>.mp3
   ```

4. **Download lyrics**:
   ```bash
   ncmdump-cli lyric <track_id> > /tmp/music/<track_id>_raw.txt
   ```
   Split the output into original and translation:
   ```python
   content = open('/tmp/music/<track_id>_raw.txt').read()
   parts = content.split('--- Translation ---')
   lrc = parts[0].strip()
   tlyric = parts[1].strip() if len(parts) > 1 else ''
   if lrc:
       open('/tmp/music/<track_id>.lrc', 'w').write(lrc)
   if tlyric:
       open('/tmp/music/<track_id>.tlyric.lrc', 'w').write(tlyric)
   ```

5. **Fetch cover URL and album info via Netease API** (batch for multiple tracks):
   ```bash
   IDS="<id1>,<id2>,..."
   C_PARAM=$(echo "$IDS" | tr ',' '\n' | \
     while read id; do echo "{\"id\":$id,\"v\":0}"; done | paste -sd',' -)
   MUSIC_U=$(cat ~/.config/ncmdump/cookie 2>/dev/null)
   curl -s "https://music.163.com/api/v3/song/detail" \
     -X POST \
     -H "Cookie: MUSIC_U=$MUSIC_U" \
     -H "Referer: https://music.163.com/" \
     -d "c=[$C_PARAM]" | python3 -c "
   import json, sys
   data = json.load(sys.stdin)
   for song in data.get('songs', []):
       sid = song['id']
       al = song.get('al', {})
       pic = al.get('picUrl', '')
       album_name = al.get('name', '')
       album_id = al.get('id', '')
       print(f'{sid}|{pic}|{album_name}|{album_id}')
   "
   ```
   Output per line: `track_id|cover_url|album_name|album_id`

6. **Ingest** (with all metadata):
   ```bash
   sf-cli write-music \
     --db-path /mnt/e/static-flow-data/lancedb-music \
     --file /tmp/music/<track_id>.mp3 \
     --id "netease-<track_id>" \
     --title "<title>" \
     --artist "<artist>" \
     --album "<album_name>" \
     --album-id "<album_id>" \
     --lyrics /tmp/music/<track_id>.lrc \
     --lyrics-translation /tmp/music/<track_id>.tlyric.lrc \
     --source netease \
     --source-id "<track_id>" \
     --tags "<comma,separated,tags>"
   ```

7. **Update cover URL** (sf-cli write-music does not accept URL directly):
   ```bash
   sf-cli db --db-path /mnt/e/static-flow-data/lancedb-music \
     update-rows songs \
     --where "id='netease-<track_id>'" \
     --set "cover_image='<cover_url>'"
   ```
   The `cover_image` field accepts either:
   - A full URL (`https://...`) — frontend uses it directly
   - A filename — frontend constructs `/api/images/<filename>`

### Flow B: Local NCM File Decrypt & Ingest
1. Decrypt: `ncmdump-cli dump <file.ncm> --output /tmp/music/`
   - Produces mp3 or flac depending on original encoding.
2. Ingest the decoded file:
   ```bash
   sf-cli write-music \
     --db-path /mnt/e/static-flow-data/lancedb-music \
     --file /tmp/music/<decoded_file> \
     --source ncm_local
   ```
   - Metadata (title, artist, album, duration) is auto-extracted from
     ID3/Vorbis tags by lofty.
   - If cover art is embedded in the audio file tags, extract it manually
     and update `cover_image` via `sf-cli db update-rows`.

### Flow C: Local mp3/flac Direct Ingest
1. Ingest directly:
   ```bash
   sf-cli write-music \
     --db-path /mnt/e/static-flow-data/lancedb-music \
     --file <audio_file> \
     --title "..." --artist "..." --album "..." --album-id "..."
   ```
2. If a cover image URL or file is available, update after ingest:
   ```bash
   sf-cli db --db-path /mnt/e/static-flow-data/lancedb-music \
     update-rows songs \
     --where "id='<song_id>'" \
     --set "cover_image='<url_or_filename>'"
   ```

## Verification
After every write, verify the record including cover and album fields:
```bash
sf-cli db --db-path /mnt/e/static-flow-data/lancedb-music \
  query-rows songs \
  --where "id='<song_id>'" \
  --columns id,title,artist,album,album_id,cover_image,format
```
Checklist:
- [ ] `title`, `artist`, `album` are populated (not "Unknown")
- [ ] `album_id` is set for Netease tracks
- [ ] `cover_image` contains a valid URL (not null/empty)
- [ ] `format` is `mp3` or `flac`

## Error Handling
- **Cookie expired**: Re-run `ncmdump-cli login qr` or `ncmdump-cli login phone`.
- **VIP-only track**: Downgrade quality to `standard` or `higher`.
- **Network timeout**: Retry up to 3 times with 5s delay.
- **Duplicate ID**: Skip unless user confirms overwrite.
- **Cover URL missing**: Fetch via Netease API (step 5 in Flow A). Never
  leave `cover_image` empty for Netease tracks.

## Output Contract
Report after each ingestion:
- `id`: Song record ID
- `title`: Song title
- `artist`: Artist name
- `album`: Album name
- `album_id`: Album ID (if available)
- `cover_image`: Cover URL (confirm non-empty)
- `format`: mp3 or flac
- `duration_ms`: Duration in milliseconds
- `status`: success or error message
