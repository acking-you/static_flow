# Local File Flows

## Flow B: NCM File Decrypt & Ingest

1. **Decrypt**: `ncmdump-cli dump <file.ncm> --output /tmp/music/`
   - Produces mp3 or flac depending on original encoding.

2. **Ingest**:
   ```bash
   sf-cli write-music \
     --db-path /mnt/e/static-flow-data/lancedb-music \
     --file /tmp/music/<decoded_file> \
     --source ncm_local
   ```
   - Metadata (title, artist, album, duration) is auto-extracted
     from ID3/Vorbis tags by lofty.
   - If cover art is embedded, extract manually and update via
     `sf-cli db update-rows`.

## Flow C: Local mp3/flac Direct Ingest

1. **Ingest**:
   ```bash
   sf-cli write-music \
     --db-path /mnt/e/static-flow-data/lancedb-music \
     --file <audio_file> \
     --title "..." --artist "..." --album "..." --album-id "..."
   ```

2. **Update cover** if available (see `common.md` → Cover Image Update).
