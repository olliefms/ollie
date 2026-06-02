# Ollie Design System

> Working guide for AI coding agents building UI in `static/driver/` (Driver PWA) and `static/fleet/` (Fleet SPA). Adapted from Linear's design language, tuned for a logistics operations tool.

## Principles

1. **Operational, not decorative.** Every pixel earns its place. No atmospheric gradients, no spotlight cards, no hero illustrations. The data is the protagonist.
2. **Glanceable status.** A driver on the road or a dispatcher scanning a list must read state in under a second. Status is carried by color + label, never color alone.
3. **Surface ladder, not shadows.** Hierarchy comes from layered surfaces with hairline borders, not drop shadows. One soft shadow token exists for popovers — that's it.
4. **Scarce accent.** Brand blue (`--color-primary`) is reserved for primary CTA, focused element, active nav, and link emphasis. Never as a section background or decorative fill.
5. **One voice, two surfaces.** Driver (mobile, single-task) and Fleet (desktop, multi-panel) share tokens, components, and rules. Density and target sizes differ; everything else does not.
6. **Light by default.** Drivers use the PWA outdoors in cabs and yards — sunlight readability matters more than a moody UI. A future dark mode is allowed but is not the canonical surface.

## Color Tokens

The existing Driver palette is canonical. Fleet extends it with a surface ladder, hairline scale, and information-density tones. Do not redefine the canonical tokens.

```css
:root {
  /* Brand & semantic — canonical, do not change */
  --color-primary: #1a56db;        /* blue-600 */
  --color-primary-dark: #1e429f;   /* blue-800 */
  --color-primary-hover: #1d4ed8;  /* blue-700 — hover on primary CTA */
  --color-primary-soft: #e0e7ff;   /* indigo-100 — focus ring tint, selected row */

  --color-danger: #e02424;
  --color-danger-soft: #fee2e2;    /* danger badge bg */
  --color-warning: #d97706;
  --color-warning-soft: #fef3c7;
  --color-success: #057a55;
  --color-success-soft: #d1fae5;
  --color-info: #1a56db;           /* alias of primary, used in info badges */
  --color-info-soft: #dbeafe;

  /* Text */
  --color-text: #111827;           /* gray-900 — body, headings */
  --color-text-muted: #6b7280;     /* gray-500 — secondary, meta */
  --color-text-subtle: #9ca3af;    /* gray-400 — tertiary, placeholder */
  --color-text-disabled: #d1d5db;  /* gray-300 */
  --color-text-inverse: #ffffff;

  /* Surface ladder (light) — canvas → surface-3 */
  --color-bg: #f9fafb;             /* gray-50 — page canvas */
  --color-surface: #ffffff;        /* surface-1 — cards, panels, table rows */
  --color-surface-2: #f3f4f6;      /* gray-100 — sub-nav, hovered row, selected tab */
  --color-surface-3: #e5e7eb;      /* gray-200 — disabled control, deep nest */

  /* Borders / hairlines */
  --color-border: #e5e7eb;         /* gray-200 — default 1px hairline */
  --color-border-strong: #d1d5db;  /* gray-300 — input border, divider on busy lists */
  --color-border-focus: #1a56db;   /* same as primary */

  /* Geometry */
  --radius-xs: 4px;     /* badges, small chips */
  --radius-sm: 6px;     /* tags */
  --radius: 8px;        /* default — buttons, inputs, cards */
  --radius-lg: 12px;    /* large panels, dispatch column */
  --radius-pill: 9999px;

  /* Elevation — used sparingly */
  --shadow: 0 1px 3px rgba(0,0,0,0.1);              /* default card shadow (kept for compat) */
  --shadow-popover: 0 8px 24px rgba(17,24,39,0.12); /* dropdowns, menus, modals */

  /* Type */
  --font: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  --font-mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
}
```

Fleet and Driver both consume this same token set. Component CSS references tokens, never raw hex.

## Typography

System font stack only. No webfont download. We rely on Apple SD Gothic / SF on Apple platforms, Segoe UI on Windows, Roboto on Android.

| Token        | Size | Weight | Line | Tracking | Use |
|--------------|------|--------|------|----------|-----|
| display      | 28px | 600    | 1.2  | -0.4px   | Page title (rare; mainly Fleet) |
| heading      | 20px | 600    | 1.3  | -0.2px   | Card title, section heading |
| subhead      | 16px | 600    | 1.4  | -0.1px   | Dense list group header |
| body         | 16px | 400    | 1.5  | 0        | Default |
| body-sm      | 14px | 400    | 1.45 | 0        | Table cells, dense Fleet rows |
| caption      | 12px | 500    | 1.4  | 0.2px    | Badge text, meta, timestamps |
| button       | 15px | 600    | 1.2  | 0        | Driver primary CTA (touch) |
| button-sm    | 14px | 500    | 1.2  | 0        | Fleet button, secondary |
| mono         | 13px | 400    | 1.5  | 0        | IDs, checksums, lat/lng, timestamps in detail views |

Rules:
- One family across both apps — system sans. Mono only for identifiers/coordinates.
- Negative tracking on display/heading; never on body.
- Never use weight 700+ for display — 600 is the cap.
- Numerals in tables/badges should use `font-variant-numeric: tabular-nums` so columns align.

## Spacing

4px base. Use these tokens; do not invent in-between values.

```
--space-1: 4px    /* tight inline */
--space-2: 8px    /* default gap */
--space-3: 12px   /* row padding (Fleet) */
--space-4: 16px   /* card interior */
--space-5: 24px   /* section gap */
--space-6: 32px   /* page padding */
--space-8: 48px   /* large empty-state padding */
```

- Driver card padding: 16px (`--space-4`).
- Fleet card padding: 16px; table row vertical padding 12px (`--space-3`); dense table 8px.
- Section separation: 24px on Driver, 16–24px on Fleet (information density wins).

## Surface & Elevation

Three-step ladder, no shadow except for popovers/modals.

| Level | Background          | Border                  | Use |
|-------|---------------------|-------------------------|-----|
| 0     | `--color-bg`        | none                    | Page canvas |
| 1     | `--color-surface`   | 1px `--color-border`    | Default card, table row, panel |
| 2     | `--color-surface-2` | 1px `--color-border`    | Hovered row, active tab, sub-nav, selected list item |
| 3     | `--color-surface-3` | 1px `--color-border-strong` | Disabled control, nested well |

Popovers (dropdowns, menus, toasts, modal): `--color-surface` background, 1px `--color-border`, `--shadow-popover`. That is the only place a real shadow is used.

## Components

### Buttons

Three variants. Min height 44px on Driver (touch), 32px on Fleet (cursor).

- **Primary** — `--color-primary` bg, white text, no border. Hover: `--color-primary-hover`. Active: `--color-primary-dark`. One per view.
- **Secondary** — `--color-surface` bg, `--color-border` 1px, `--color-text` text. Hover: `--color-surface-2`.
- **Ghost / tertiary** — transparent bg, no border, `--color-text` text. Hover: `--color-surface-2`. Used for toolbar actions.
- **Destructive** — `--color-danger` bg, white text. Use only for irreversible actions; confirm with a modal.

Disabled: `opacity: 0.5; cursor: not-allowed;`. Never gray out by changing color tokens.

Focus ring: 3px `rgba(26,86,219,0.15)` outer + 1px `--color-primary` border. Always visible on keyboard focus.

### Inputs

- 1.5px border in `--color-border-strong`, 8px radius, white surface.
- Focus: border becomes `--color-primary`, 3px soft ring `rgba(26,86,219,0.15)`.
- Error: border `--color-danger`, helper text below in `--color-danger`, 13px.
- Driver: min-height 44px. Fleet: 32px (compact) or 36px (default).
- Labels above the field, weight 500, `body-sm`, color `--color-text`. Never use placeholder as a label.

### Badges (status pills)

Status is the workhorse of an ops tool. Use a soft-bg + saturated-text pattern so badges don't shout.

| Status        | Background             | Text                  | Trip statuses |
|---------------|------------------------|-----------------------|---------------|
| Active / Info | `--color-info-soft`    | `--color-primary-dark`| `in_transit`, `dispatched` |
| Success / Done| `--color-success-soft` | `--color-success`     | `delivered`, `completed` |
| Warning / Pending | `--color-warning-soft` | `--color-warning` | `pending` |
| Danger / Failed | `--color-danger-soft` | `--color-danger`     | `cancelled` |
| Neutral / Idle | `--color-surface-2`   | `--color-text-muted`  | `assigned`, `planned` |
| Scheduled trips | `--color-badge-scheduled-bg` | `--color-badge-scheduled-text` | `scheduled` |

The scheduled pair (`--color-badge-scheduled-bg: #f3e8ff`, `--color-badge-scheduled-text: #6b21a8`) uses a purple hue that has no semantic alias in the core palette. These tokens live in `base.css` and must not be used outside badge context.

Shape: `--radius-pill`, `caption` type, padding 2px 8px. Always include a text label — never color-only.

### Cards

`--color-surface` bg, 1px `--color-border`, `--radius` 8px, 16px interior padding. Card title: `heading`. Card body: `body` or `body-sm`. No shadow on cards by default (the existing `--shadow` is preserved for back-compat with the Driver app but new code should rely on the hairline border instead).

### Tables (Fleet)

The Fleet leans on tables for trips, loads, and assignments.

- Header row: `--color-surface-2` bg, `caption` type uppercase, `--color-text-muted`, sticky on scroll.
- Body row: `--color-surface` bg, 1px `--color-border` bottom, 12px vertical padding.
- Hover row: `--color-surface-2`. Selected row: `--color-primary-soft` bg, left border 2px `--color-primary`.
- Numeric columns: right-aligned, `tabular-nums`.
- Empty state: 48px padding, `--color-text-muted`, single short sentence + optional CTA.

### Forms

- Stack vertically. Two-column layout only on Fleet when fields are obviously paired (lat/lng, start/end).
- Required fields marked with `*` after label, `--color-danger`.
- Submit button right-aligned in a footer row separated by 1px `--color-border`.

### Navigation

- **Driver** — bottom tab bar on mobile (3–4 tabs max), top app bar with current view title and one optional action button. Active tab: `--color-primary` icon + label, others `--color-text-muted`.
- **Fleet** — left sidebar (220px) with grouped link list, top bar with breadcrumb + global actions. Active link: `--color-surface-2` bg, `--color-text` text, 2px `--color-primary` left border.

### Dialogs / modals

Center on Fleet; bottom-sheet on Driver mobile. Surface 1, 1px border, 12px radius, `--shadow-popover`. Backdrop `rgba(17,24,39,0.4)`.

## Layout

### Driver PWA (mobile-first)

- Single column. Max content width 480px on tablets.
- Top app bar 56px, bottom tab bar 64px (with safe-area inset).
- Cards full-width minus 16px page gutter.
- Touch targets ≥ 44×44px. Buttons full-width by default in primary action zones.
- One primary action per screen. Secondary actions in a kebab menu or below the fold.

### Fleet (desktop SPA)

- Three-region shell: sidebar (220px fixed), top bar (48px fixed), content (fluid).
- Content max-width: none — Fleet is meant to fill wide monitors. Set per-page max widths only when the content is genuinely narrow (forms, detail panes).
- Multi-panel views (list + detail) use a 2-pane split, resizable where useful, with a 1px `--color-border` divider.
- Information density: prefer 14px body in lists/tables, 16px in detail views.

## Icons

Use [Lucide](https://lucide.dev) (MIT, SVG, tree-shake-friendly) at 20px (Fleet) and 24px (Driver). Stroke width 2. Color inherits from text (`currentColor`). No filled/duotone variants. No emoji as UI icons.

## Motion

Restrained. Two timings only:

- **Quick** — 150ms `ease-out` — hover, focus, button press, toggle.
- **Modal** — 200ms `ease-out` — dialog/sheet open, dropdown reveal.

Never animate layout-shifting properties on the critical path. No bounce, no scale-in card grids, no parallax.

## Do

- Reuse tokens. Edit `base.css` rather than inlining hex.
- Lead with the data: tables, lists, status — minimal chrome around them.
- Use status badges with both color and label.
- Keep one primary CTA per view.
- Test Driver in bright sunlight (or a 1000 nit monitor at full brightness) — contrast must hold.

## Don't

- Don't introduce a second brand color. Blue is it.
- Don't use drop shadows on cards or panels — surfaces + hairlines do that work.
- Don't use color alone to communicate state.
- Don't use pill-shaped buttons. Pills are for status badges and segmented toggles only.
- Don't put marketing-style hero illustrations into the apps.
- Don't ship a custom webfont. System sans only.
- Don't use placeholder text as a label.

## File Locations

- Driver tokens & base: `static/driver/css/base.css`
- Driver components: `static/driver/css/components.css`
- Fleet (when added in v1.4): `static/fleet/css/base.css`, `static/fleet/css/components.css` — must import the same tokens.

When extending, prefer adding a new component class in `components.css` over inline styles. When a component needs a new token, add it to `base.css` and document it here.
