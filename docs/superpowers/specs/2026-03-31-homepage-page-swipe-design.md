# StaticFlow Homepage Page-Swipe Redesign

## Goal

Split the homepage's two terminal containers (system_info.sh and explore.sh)
into two horizontally swipeable pages with smooth transitions, a floating
page indicator, and proper mobile/desktop adaptation.

## Design Decisions

- **Direction:** Horizontal swipe (avoids conflict with vertical content scrolling)
- **Trigger:** Touch drag (mobile) + mouse drag (PC) + bottom dot indicator click
- **Indicator:** iOS-style floating pill with terminal-cursor-shaped dots
- **Approach:** CSS scroll-snap + PC mouse drag enhancement (Approach C)
- **Visual:** Parallax depth shift on inactive page + acrylic glass indicator

## Page Structure

```
<div class="home-page">
    <!-- Floating page indicator (fixed, always visible) -->
    <PageDots active_page={0|1} on_click={switch} />

    <!-- Horizontal scroll-snap container -->
    <div class="page-slider">
        <!-- Page 0: system_info.sh -->
        <div class="page-slide">
            Hero Terminal + LLM Banner + Stats
        </div>

        <!-- Page 1: explore.sh -->
        <div class="page-slide">
            Explore Terminal (articles + music + tech + social)
        </div>
    </div>
</div>
```

## CSS Components

### Page Slider

```css
.page-slider {
  display: flex;
  overflow-x: auto;
  scroll-snap-type: x mandatory;
  scroll-behavior: smooth;
  scrollbar-width: none;
  -webkit-overflow-scrolling: touch;
}
.page-slider::-webkit-scrollbar { display: none; }

.page-slide {
  min-width: 100%;
  scroll-snap-align: start;
  scroll-snap-stop: always;
  flex-shrink: 0;
  transition: opacity 0.5s var(--ease-spring),
              transform 0.5s var(--ease-spring);
}
.page-slide.inactive {
  opacity: 0.6;
  transform: scale(0.97);
}
```

### Page Dots Indicator

Floating acrylic glass pill at bottom center. Dots are pill-shaped (terminal
cursor style), active dot pulses with primary color glow.

```css
.page-dots {
  position: fixed;
  bottom: 1.5rem;
  left: 50%;
  transform: translateX(-50%);
  z-index: 50;
  display: flex;
  align-items: center;
  gap: 0.625rem;
  padding: 0.5rem 0.875rem;
  border-radius: 9999px;
  background: rgba(var(--surface-rgb), 0.72);
  backdrop-filter: blur(var(--acrylic-blur)) saturate(var(--acrylic-saturate));
  border: 1px solid rgba(var(--primary-rgb), 0.15);
  box-shadow: var(--shadow-4);
}

.page-dot {
  width: 1.75rem;
  height: 0.375rem;
  border-radius: 9999px;
  background: var(--muted);
  opacity: 0.35;
  transition: all 0.4s var(--ease-spring);
  cursor: pointer;
}

.page-dot.active {
  background: var(--primary);
  opacity: 1;
  box-shadow: 0 0 8px rgba(var(--primary-rgb), 0.5);
  animation: dot-pulse 2s ease-in-out infinite;
}

@keyframes dot-pulse {
  0%, 100% { box-shadow: 0 0 8px rgba(var(--primary-rgb), 0.5); }
  50% { box-shadow: 0 0 14px rgba(var(--primary-rgb), 0.8); }
}
```

### Dark Mode Enhancements

```css
[data-theme="dark"] .page-dots,
[theme="dark"] .page-dots {
  border-color: rgba(var(--primary-rgb), 0.25);
  box-shadow: var(--shadow-4), 0 0 12px rgba(var(--primary-rgb), 0.1);
}

[data-theme="dark"] .page-dot.active,
[theme="dark"] .page-dot.active {
  box-shadow: 0 0 10px rgba(var(--primary-rgb), 0.7);
}
```

### Responsive

```css
@media (max-width: 767px) {
  .page-dots { bottom: 1rem; }
}
```

## Interaction Model

### Mobile (touch)

- Browser-native scroll-snap handles drag + inertia + snap
- No custom touch event code needed
- `scroll` event on `.page-slider` updates `active_page` state

### PC (mouse drag enhancement)

- `mousedown` on `.page-slider`: record `startX` and `startScrollLeft`
- `mousemove`: set `scrollLeft = startScrollLeft - (clientX - startX)`
- `mouseup`: release, scroll-snap auto-snaps to nearest page
- Drag threshold: > 5px movement = drag (suppress click), otherwise = click
- Cursor: `grab` default, `grabbing` while dragging

### Dot indicator state sync

- `scroll` event: `active_page = Math.round(scrollLeft / containerWidth)`
- Throttle via `requestAnimationFrame`
- `active_page` stored in `use_state`, drives dot highlight + `.inactive` class

### Dot click

- `container.scrollTo({ left: pageIndex * containerWidth, behavior: 'smooth' })`

## Responsive Adaptation

### Mobile (< 768px)

- `.page-slide` inner padding: `px-4`
- Dots: `bottom-4`
- Touch drag: native scroll-snap
- Each slide scrolls vertically independently

### Desktop (>= 768px)

- `.page-slide` inner content: `max-w-5xl mx-auto`
- Dots: `bottom-6`
- Mouse drag enhancement active
- Each slide scrolls vertically independently

### Height

- `.page-slider`: height determined by tallest slide (no forced `min-h-screen`)
- `.page-slide`: height by content (not forced `h-screen`)
- Two pages may have different heights — the container stretches to the tallest

## Files Changed

| File | Change |
|------|--------|
| `frontend/src/pages/home.rs` | Restructure HTML into scroll-snap container + two slides, add PageDots inline component, add scroll/mouse event listeners |
| `frontend/input.css` | Add `.page-slider`, `.page-slide`, `.page-dots`, `.page-dot` styles |

## Files NOT Changed

- `frontend/src/router.rs` — no new routes
- `frontend/src/api.rs` — no API changes
- `frontend/src/i18n/zh_cn.rs` — existing keys sufficient

## Risks & Assumptions

- **Assumption:** CSS scroll-snap is supported in all target browsers (> 95% global support).
- **Assumption:** Mini player (`z-[70]`, fixed bottom-right) does not overlap with page dots (`z-50`, fixed bottom-center).
- **Risk:** PC mouse drag may interfere with text selection inside terminal. Mitigated by only activating drag on the `.page-slider` container itself, not on child content.
- **Risk:** Two slides with different heights — the slider container height is determined by the taller slide. The shorter slide will have empty space at the bottom. This is acceptable.
