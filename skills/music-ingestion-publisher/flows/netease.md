# Netease Cloud Music Flows

Primary source for music ingestion. If download fails with 403/VIP/copyright
error, fall back to Bilibili (see `bilibili.md`) while still using Netease
for lyrics and metadata.

## Flow A: Online Search & Download

1. **Search**: `ncmdump-cli search "<keyword>" --type track --limit 10`

2. **Track info**: `ncmdump-cli info <track_id>`
   - Note: does NOT print cover URL (see step 5).

3. **Download audio**:
   ```bash
   ncmdump-cli download <track_id> --quality exhigh \
     --output /tmp/music/<track_id>.mp3
   ```
   If download fails (VIP/copyright) → switch to Bilibili Flow E.

4. **Lyrics**:
   ```bash
   ncmdump-cli lyric <track_id> > /tmp/music/<track_id>_raw.txt
   ```
   Split at `--- Translation ---`:
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

5. **Fetch cover URL + album info** (batch):
   ```bash
   MUSIC_U=$(python3 -c "import json; print(json.load(open(\
     '/home/ts_user/.config/ncmdump/session.json'))['MUSIC_U'])")
   C_PARAM=$(echo "<id1>,<id2>,..." | tr ',' '\n' | \
     while read id; do echo "{\"id\":$id,\"v\":0}"; done | paste -sd',' -)
   curl -s "https://music.163.com/api/v3/song/detail" \
     -X POST -H "Cookie: MUSIC_U=$MUSIC_U" \
     -H "Referer: https://music.163.com/" \
     -d "c=[$C_PARAM]" | python3 -c "
   import json, sys
   for s in json.load(sys.stdin).get('songs', []):
       al = s.get('al', {})
       print(f'{s[\"id\"]}|{al.get(\"picUrl\",\"\")}|{al.get(\"name\",\"\")}|{al.get(\"id\",\"\")}')
   "
   ```

6. **Ingest** (with cover URL from step 5):
   ```bash
   sf-cli write-music \
     --db-path /mnt/e/static-flow-data/lancedb-music \
     --file /tmp/music/<track_id>.mp3 \
     --id "netease-<track_id>" \
     --title "<title>" --artist "<artist>" \
     --album "<album>" --album-id "<album_id>" \
     --cover-url "<cover_url_from_step5>" \
     --lyrics /tmp/music/<track_id>.lrc \
     --lyrics-translation /tmp/music/<track_id>.tlyric.lrc \
     --source netease --source-id "<track_id>"
   ```

7. **Verify** (see `common.md`).
   If cover was not available at ingest, update it separately (see `common.md`).

## Flow D: Bulk Album Ingestion

`ncmdump-cli` has **no** `artist` subcommand — use Netease HTTP API.

### Step 1: Find artist
```bash
ncmdump-cli search "<artist>" --type artist --limit 10
```

### Step 2: List albums
```bash
curl -s "https://music.163.com/api/artist/albums/<artist_id>?limit=50" \
  -H "Cookie: MUSIC_U=$MUSIC_U" -H "Referer: https://music.163.com/" \
  | python3 -c "
import json, sys
for a in json.load(sys.stdin).get('hotAlbums', []):
    print(f'[{a[\"id\"]}] {a[\"name\"]} - {a.get(\"size\",0)} tracks')
"
```

### Step 3: Album tracks
```bash
curl -s "https://music.163.com/api/v1/album/<album_id>" \
  -H "Cookie: MUSIC_U=$MUSIC_U" -H "Referer: https://music.163.com/" \
  | python3 -c "
import json, sys
data = json.load(sys.stdin)
for s in data.get('songs', []):
    print(f'  {s[\"id\"]}|{s[\"name\"]}')
"
```

### Step 4: Check existing, then bulk ingest
Skip songs already in DB. For each new track, follow Flow A steps 3-7.
If any track fails download (copyright), collect it for Bilibili Flow F.
