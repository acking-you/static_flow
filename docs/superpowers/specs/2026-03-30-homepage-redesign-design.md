# StaticFlow Homepage Redesign

## Goal

Transform the homepage from a terminal-only showcase into a content discovery
entry point, while preserving the terminal aesthetic as the hero visual identity.

## Current Problems

1. Terminal metaphor flattens information hierarchy — everything is `terminal-line`
2. No content preview — visitors must navigate away to discover articles/music
3. Navigation buried in tab switcher (Navigation / Social)
4. Stats panel pushed below the fold
5. Tech stack section takes disproportionate space for low visitor value
6. LLM Access promotion awkwardly embedded in terminal flow

## Design: Terminal Hub (Approach A)

### Page Structure (top to bottom)

```
1. Hero Terminal (simplified)
2. LLM/Kiro Access Banner (independent card)
3. Stats Bar (5-card grid)
4. Recent Articles (4 ArticleCards)
5. Recent Music (4 SongMiniCards)
6. Tech Stack (inline logo strip)
7. Social + GitHub Wrapped (footer zone)
```

### Section 1: Hero Terminal

Retain terminal outer shell (macOS dots + title bar). Simplify interior:

- Terminal header (three dots + `staticflow@local` title) — unchanged
- `$ cat avatar` + avatar with hover-spin — unchanged
- `$ echo $MOTTO` + motto output — unchanged
- `$ cat README` + one-line intro — unchanged
- Open source line with GitHub link — unchanged
- CTA button group — flat layout, no tabs:
  `[文章] [归档] [音乐] [图片] [搜索] [Admin]`
- Blinking cursor — unchanged

**Removed from Hero:**
- `HomeTab` enum and tab switching logic
- LLM Access promotion block → moved to Section 2
- Social links + GitHub Wrapped → moved to Section 7
- Media Hub button group (redundant with CTA) → merged into CTA

Estimated reduction: ~270 lines → ~120 lines of HTML.

### Section 2: LLM/Kiro Access Banner

Independent card between Hero and Stats.

- Style: `bg-[var(--surface)]`, `border-l-[4px] border-l-[var(--primary)]`,
  `rounded-lg`, `shadow-[var(--shadow-2)]` — matches Stats card design language
- Layout: left side = hint text (`t::LLM_ACCESS_HINT`),
  right side = two buttons (`btn-terminal-accent` for Key, `btn-terminal` for Kiro)
- Responsive: desktop = text and buttons on same row; mobile = stacked

### Section 3: Stats Bar

Position moved from bottom to directly below Banner. No functional changes.

- Section title: terminal prompt style (`$ neofetch --stats`)
- Grid: `grid-cols-2 sm:grid-cols-3 lg:grid-cols-5` — unchanged
- 5 compact cards with existing `system-panel-compact` style — unchanged
- Data: `fetch_site_stats()` + `fetch_images_page(1,0)` — unchanged
- Music count now comes from the shared `fetch_songs(4,0)` call (see Section 5)
- Skeleton loading state — unchanged
- Clickable cards linking to respective pages — unchanged

### Section 4: Recent Articles

New section. Terminal-style title + ArticleCard grid + "view all" link.

- Section title: `$ ls ./recent-articles`
- Data: `fetch_articles(None, None, Some(4), Some(0))` — existing API
- Component: reuse existing `ArticleCard` (`frontend/src/components/article_card.rs`)
- Grid: `grid-cols-1 md:grid-cols-2`, gap = `var(--space-card-gap)`
- Loading state: 4 skeleton cards
- "查看全部 →" link → `Route::LatestArticles`
- New state: `let recent_articles = use_state(|| Vec::<ArticleListItem>::new());`
- Fetch fires in the same `use_effect_with` as stats (parallel)

### Section 5: Recent Music

New section. Terminal-style title + SongMiniCard grid + "view all" link.

- Section title: `$ ls ./recent-music`
- Data: `fetch_songs(Some(4), Some(0), None, None, None)` — existing API,
  shared with Stats (`.total` for stats count, `.songs` for preview cards)
- Component: inline `SongMiniCard` defined in `home.rs` (no separate file)
  - Cover image via `ImageWithLoading` (with spinner loading state — see below)
  - Missing cover: `IconName::Music` placeholder
  - Title + artist + duration
  - Click → `Route::MusicPlayer { id }`
- Grid: `grid-cols-2 md:grid-cols-4`
- Loading state: 4 skeleton cards
- "查看全部 →" link → `Route::MediaAudio`
- New state: reuse `SongListItem` vec from stats fetch (already fetching songs)

### Section 6: Tech Stack Logo Strip

Compressed from `command-list` to inline flex row.

- Section title: terminal prompt style (`$ cat ./tech-stack`)
- Layout: `flex flex-wrap gap-3`, single row
- Each item: small logo image (`ImageWithLoading`) + name label, clickable → docs URL
- Reuse existing `tech_stack` array data
- Style: subtle, low visual weight

### Section 7: Social + GitHub Wrapped

Page footer zone, below tech stack.

- Section title: `$ cat ./social`
- Social buttons: GitHub + Bilibili — reuse existing `btn-fluent-icon` style, flat layout
- GitHub Wrapped: reuse existing `GithubWrappedSelector` component, moved from tab content
- Low visual weight, page closure

## Cross-Cutting Change: ImageWithLoading Spinner

**File:** `frontend/src/components/image_with_loading.rs`

Replace `animate-pulse` gradient skeleton with centered `LoadingSpinner` (size: Small).

- Loading state: `bg-[var(--surface-alt)]` background + centered `LoadingSpinner`
  (import from `crate::components::loading_spinner::{LoadingSpinner, SpinnerSize}`)
- Loaded state: spinner disappears, image fades in via existing
  `opacity-0 → opacity-100` transition (500ms) — unchanged
- This change benefits all pages using `ImageWithLoading` globally

## Files Changed

| File | Change |
|------|--------|
| `frontend/src/pages/home.rs` | Major rewrite: simplify Hero, remove tabs, add 4 new sections, add data fetches |
| `frontend/src/components/image_with_loading.rs` | Replace pulse skeleton with LoadingSpinner |
| `frontend/src/i18n/zh_cn.rs` | Add new section title strings (recent articles, recent music, view all, etc.) |

## Files NOT Changed

- `frontend/src/components/article_card.rs` — reused as-is
- `frontend/src/components/stats_card.rs` — reused as-is
- `frontend/src/components/loading_spinner.rs` — reused as-is
- `frontend/src/api.rs` — all needed APIs already exist
- `frontend/src/router.rs` — no new routes
- `frontend/input.css` — existing styles sufficient (terminal-line, system-panel-compact, btn-terminal, etc.)

## New i18n Keys Needed

```
home::CMD_SHOW_RECENT_ARTICLES  // "ls ./recent-articles"
home::CMD_SHOW_RECENT_MUSIC     // "ls ./recent-music"
home::CMD_SHOW_TECH_STACK       // "cat ./tech-stack"
home::CMD_SHOW_SOCIAL           // already exists
home::BTN_VIEW_ALL_ARTICLES     // "查看全部 →"
home::BTN_VIEW_ALL_MUSIC        // "查看全部 →"
```

## Risks & Assumptions

- **Assumption:** `fetch_articles` with `limit=4, offset=0` returns newest-first.
  Current backend sorts by `created_at` desc — verified in `latest_articles.rs` usage.
- **Assumption:** `SongListItem` has enough fields (title, artist, cover_image, duration_ms)
  for the mini card. Verified from API struct definition.
- **Risk:** Adding 2 more API calls on homepage load. Mitigated by firing all fetches
  in parallel within one `use_effect_with`. Total: 4 parallel requests (stats +
  images + songs + articles). The songs fetch is shared between stats count and
  music preview section.
- **Risk:** `ImageWithLoading` spinner change is global. All existing usages will switch
  from pulse to spinner. This is intentional and desired.
