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

### Reading the session cookie
The session is stored as JSON, **not** a plain text cookie file:
```bash
# Correct way to read MUSIC_U:
MUSIC_U=$(python3 -c "import json; print(json.load(open('/home/ts_user/.config/ncmdump/session.json'))['MUSIC_U'])")
```
`~/.config/ncmdump/cookie` does **not** exist. Always read from `session.json`.

### ncmdump-cli available subcommands
```
dump      Decrypt NCM files to MP3/FLAC
login     Set login cookie
logout    Clear saved session
search    Search tracks/albums/artists/playlists
info      Show track details
lyric     Get track lyrics
download  Download a track
playlist  Show playlist details
me        Show current user info
```
There is **no** `artist` subcommand. Use the Netease HTTP API directly to
list an artist's albums or an album's tracks (see Flow D below).

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
   MUSIC_U=$(python3 -c "import json; print(json.load(open('/home/ts_user/.config/ncmdump/session.json'))['MUSIC_U'])")
   C_PARAM=$(echo "<id1>,<id2>,..." | tr ',' '\n' | \
     while read id; do echo "{\"id\":$id,\"v\":0}"; done | paste -sd',' -)
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

### Flow D: Bulk Album Ingestion via Netease API

Use this flow when ingesting a full album or all albums of an artist.
`ncmdump-cli` has **no** `artist` subcommand — use the Netease HTTP API.

#### Step 1: Find artist ID
```bash
ncmdump-cli search "<artist_name>" --type artist --limit 10
```
Search results may include multiple artists with similar names.
**Always verify** the correct artist by checking their album list (step 2).
Do not rely on the first result.

#### Step 2: List artist albums
```bash
MUSIC_U=$(python3 -c "import json; print(json.load(open('/home/ts_user/.config/ncmdump/session.json'))['MUSIC_U'])")
curl -s "https://music.163.com/api/artist/albums/<artist_id>?limit=50&offset=0" \
  -H "Cookie: MUSIC_U=$MUSIC_U" \
  -H "Referer: https://music.163.com/" | python3 -c "
import json, sys
data = json.load(sys.stdin)
for a in data.get('hotAlbums', []):
    print(f'[{a[\"id\"]}] {a[\"name\"]} - {a.get(\"size\",0)} tracks')
"
```
If the artist has >50 albums, paginate with `&offset=50`, `&offset=100`, etc.

#### Step 3: Get album track list
```bash
curl -s "https://music.163.com/api/v1/album/<album_id>" \
  -H "Cookie: MUSIC_U=$MUSIC_U" \
  -H "Referer: https://music.163.com/" | python3 -c "
import json, sys
data = json.load(sys.stdin)
album = data.get('album', {})
print(f'Album: {album[\"name\"]} | AlbumID: {album[\"id\"]}')
for s in data.get('songs', []):
    print(f'  {s[\"id\"]}|{s[\"name\"]}')
"
```

#### Step 4: Check existing songs before bulk ingest
Avoid re-ingesting tracks already in the DB:
```bash
sf-cli db --db-path /mnt/e/static-flow-data/lancedb-music \
  query-rows songs --columns id --limit 1000 2>/dev/null | python3 -c "
import sys, re
ids = set()
for line in sys.stdin:
    m = re.search(r'netease-(\d+)', line)
    if m: ids.add(int(m.group(1)))
print(f'Existing: {len(ids)} songs')
# Save to file for use in ingestion script
with open('/tmp/existing_ids.txt', 'w') as f:
    f.write('\n'.join(str(i) for i in ids))
"
```

#### Step 5: Bulk ingest with Python script
For albums with many tracks, use a Python script (see template below).
The script handles: skip-existing, download, lyrics, cover fetch, ingest, cover update.

```python
#!/usr/bin/env python3
import json, os, subprocess, time, urllib.request

NCD = "./tools/ncmdump-rs/target/release/ncmdump-cli"
SF  = "./target/release/sf-cli"
DB  = "/mnt/e/static-flow-data/lancedb-music"
TMP = "/tmp/music"
os.makedirs(TMP, exist_ok=True)

SESSION = json.load(open("/home/ts_user/.config/ncmdump/session.json"))
MUSIC_U = SESSION["MUSIC_U"]

def api(url):
    req = urllib.request.Request(url, headers={
        "Cookie": f"MUSIC_U={MUSIC_U}",
        "Referer": "https://music.163.com/",
        "User-Agent": "Mozilla/5.0",
    })
    with urllib.request.urlopen(req, timeout=15) as r:
        return json.loads(r.read())

def existing_ids():
    r = subprocess.run(
        [SF, "db", "--db-path", DB, "query-rows", "songs",
         "--columns", "id", "--limit", "1000"],
        capture_output=True, text=True
    )
    ids = set()
    import re
    for line in r.stdout.splitlines():
        m = re.search(r'netease-(\d+)', line)
        if m: ids.add(int(m.group(1)))
    return ids

# ... (download, lyrics, ingest functions as in Flow A steps 3-7)
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

## Vector Embedding Backfill
After bulk-importing songs (or upgrading from a pre-vector schema), run:
```bash
sf-cli embed-songs --db-path /mnt/e/static-flow-data/lancedb-music
```
This command:
- Auto-migrates the songs table to add `vector_en`/`vector_zh` columns if missing.
- Scans all songs where `vector_en IS NULL`.
- Generates bilingual embeddings from `searchable_text` (BGE-Small-EN 384d + BGE-Small-ZH 512d).
- Batch-updates vectors via `merge_insert` (only vector columns are touched).

After backfill, rebuild vector indexes:
```bash
sf-cli ensure-indexes --db-path /mnt/e/static-flow-data/lancedb
```

Note: `write-music` now auto-generates vectors at ingest time, so this command
is only needed for songs imported before vector support was added.

## Error Handling
- **Cookie expired**: Re-run `ncmdump-cli login qr` or `ncmdump-cli login phone`.
- **VIP-only track**: Downgrade quality to `standard` or `higher`.
- **Network timeout**: Retry up to 3 times with 5s delay.
- **Duplicate ID**: Skip unless user confirms overwrite.
- **Cover URL missing**: Fetch via Netease API (step 5 in Flow A). Never
  leave `cover_image` empty for Netease tracks.
- **Downloaded file is M4A (not MP3)**: Some tracks download as AAC/M4A
  even with a `.mp3` extension. Detect with `file <path>` — if output
  contains "Apple iTunes ALAC/AAC-LC", convert before ingesting:
  ```bash
  # Detect
  file /tmp/music/<track_id>.mp3
  # Convert to MP3 (requires ffmpeg)
  ffmpeg -y -i /tmp/music/<track_id>.mp3 \
    -codec:a libmp3lame -q:a 2 \
    /tmp/music/<track_id>_conv.mp3
  # Then ingest the _conv.mp3 file
  ```
  sf-cli only accepts `mp3` and `flac`. M4A/AAC will fail with
  `"Mpeg: File contains an invalid frame"`.
- **`ncmdump-cli` has no `artist` subcommand**: Use the Netease HTTP API
  directly (see Flow D). The error message is:
  `error: unrecognized subcommand 'artist'`

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
