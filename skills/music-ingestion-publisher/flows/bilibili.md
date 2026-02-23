# Bilibili Flows

Use Bilibili as audio source when Netease Cloud Music returns copyright/VIP
errors. Lyrics are still fetched from Netease via `ncmdump-cli lyric`.

## Preconditions

1. `ncmdump-cli bili-me` — verify login (session lasts ~6 months)
   - If not logged in: `ncmdump-cli bili-login` (terminal QR scan)
2. `ffmpeg -version` — required for DASH m4s → mp3/flac conversion

## Flow E: Single Song

1. **Search**: `ncmdump-cli bili-search "<song> <artist>" --limit 10`
   - Results: `[bvid] author - title (duration)`
   - Selection priority: Hi-Res/无损 audio > official MV > first match
   - Skip: reaction videos, covers, short clips (<1min)

2. **Video info**: `ncmdump-cli bili-info <bvid>`
   - Note the cover URL (`Cover:` line) for step 6.

3. **Download audio** (320kbps MP3):
   ```bash
   ncmdump-cli bili-download <bvid> --format mp3 \
     --output /tmp/music/bili-<bvid>.mp3
   ```
   For FLAC (大会员 only): `--format flac`

4. **Lyrics** (Bilibili has no lyrics API):
   - If Netease track_id is known:
     `ncmdump-cli lyric <netease_track_id>`
   - Split output at `--- Translation ---` into `.lrc` and `.tlyric.lrc`
   - If no track_id: skip lyrics or provide manually.

5. **Ingest**:
   ```bash
   sf-cli write-music \
     --db-path /mnt/e/static-flow-data/lancedb-music \
     --file /tmp/music/bili-<bvid>.mp3 \
     --id "bilibili-<bvid>" \
     --title "<song_title>" \
     --artist "<actual_artist>" \
     --source bilibili \
     --source-id "<bvid>" \
     --lyrics /tmp/music/<id>.lrc \
     --lyrics-translation /tmp/music/<id>.tlyric.lrc
   ```

6. **Update cover** (MUST use `https://`, not `http://`):
   ```bash
   # Bilibili API returns http:// URLs — always convert to https://
   sf-cli db --db-path /mnt/e/static-flow-data/lancedb-music \
     update-rows songs \
     --where "id='bilibili-<bvid>'" \
     --set "cover_image='https://i0.hdslb.com/bfs/archive/...'"
   ```

7. **Verify** (see `common.md`).

## Flow F: Batch Ingestion

For ingesting multiple songs (e.g. Netease copyright-blocked list):

1. Prepare list: `(song_name, artist, netease_track_id)` per song.
2. Check existing: `sf-cli db query-rows songs --columns id --limit 1000`
3. For each song not in DB:
   a. Search → select best bvid
   b. Download audio (320k mp3)
   c. Fetch lyrics from Netease via track_id
   d. Ingest + update cover (https://)
4. After batch: verify all records, run vector backfill if needed.

Rate limit: 1-2 second delay between downloads to avoid throttling.

## Notes

- 大会员: up to 192K AAC or FLAC. Non-VIP: 128K/192K AAC max.
- Audio pipeline: DASH m4s → ffmpeg → mp3 (320kbps CBR) or flac.
- Cover = video thumbnail (`video_detail.pic`). Always https://.
- Album metadata usually unavailable from Bilibili; use song title or
  leave album field as artist name.
- Multi-part videos: only first part audio is downloaded by default.
