---
name: keria-plus-pickup
description: Use when redeem-code pickup from plus.keria.cc.cd needs to be automated into a downloaded ZIP and an unpacked local directory, especially for plus_json bundles.
---

# Keria Plus Pickup

Use this skill to automate bulk pickup from `plus.keria.cc.cd`.

The site frontend posts a multipart form directly to `/pickup` and the response
body is already the ZIP file. There is no extra signed download URL step in the
basic pickup flow.

## When To Use

- You already have valid pickup codes.
- You want a local ZIP plus an unpacked directory.
- The target site is `plus.keria.cc.cd` or a compatible deployment.

## Request Shape

Required form fields:

- `codes`
- `output_format`
- `progress_id`

For Plus JSON pickup, use:

- `output_format=plus_json`

## Helper Script

Show help:

```bash
bash skills/keria-plus-pickup/scripts/fetch_pickup_bundle.sh --help
```

Typical use:

```bash
bash skills/keria-plus-pickup/scripts/fetch_pickup_bundle.sh \
  --codes-file /path/to/codes.txt \
  --output-dir /path/to/output
```

With explicit base URL:

```bash
bash skills/keria-plus-pickup/scripts/fetch_pickup_bundle.sh \
  --base-url https://plus.keria.cc.cd \
  --codes-file /path/to/codes.txt \
  --output-dir /path/to/output
```

Dry run:

```bash
bash skills/keria-plus-pickup/scripts/fetch_pickup_bundle.sh \
  --codes-file /path/to/codes.txt \
  --output-dir /path/to/output \
  --dry-run
```

## Output

The script writes:

- `pickup-plus-json.zip`
- `pickup.headers.txt`
- `unpacked/`

If the archive contains `取件结果.txt`, read it first to confirm success and
failure counts.

## Notes

- The script sends browser-like `Origin`, `Referer`, and `User-Agent` headers.
- One line per code is preferred.
- Default output format is `plus_json`.
