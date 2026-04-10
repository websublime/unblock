# ://unblock — Brand Guide

**Brutalist terminal identity for dependency-aware task tracking.**

| | |
|---|---|
| **Version** | 1.0.0 |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Date** | March 2026 |
| **Ecosystem** | websublime :// |

---

## 1. Brand Concept

The brand IS the terminal. Everything is monospace. The `://` protocol mark is the shared identity across the websublime ecosystem, with each product distinguished by its accent colour.

**://unblock** is a tool, not an app. It speaks in commands, not interfaces. The primary user is an AI agent — the desktop app is the human's window into what agents see.

### Core Principles

- **All monospace** — JetBrains Mono preferred, system monospace fallback. Zero sans-serif anywhere in the brand.
- **Dark-first** — Night (#0F172A) is the primary background. The brand was born in the terminal. Light is the adaptation.
- **Always lowercase** — Terminals are lowercase. URLs are lowercase. The brand is lowercase. Never "Unblock" or "UNBLOCK".
- **Tool energy** — Every interaction reads as a command: `://ready`, `://claim #42`, `://close`.

---

## 2. Logo

### 2.1 Primary Lockup

The `://` in bold 700 monospace, followed by `unblock` in regular weight monospace. This weight contrast is the only typographic hierarchy.

```
://unblock
^^^         bold 700, Violet (#A78BFA)
   ^^^^^^^  regular 400, text colour (white on dark, black on light)
```

### 2.2 Icon Mark

The `://` alone, in bold monospace. Used for favicon, app icon, social avatar, dock icon, menubar, tray.

### 2.3 Colour Contexts

| Context | :// colour | Wordmark colour | Background |
|---|---|---|---|
| On dark (default) | Violet #A78BFA | White #F8FAFC | Night #0F172A |
| On light | Deep #7C3AED | Black #0F172A | Snow #F8FAFC |
| On violet | Abyss #1E1B4B | Abyss #1E1B4B | Violet #A78BFA |

---

## 3. Label System

The killer feature of the brand. Every tool is a "route" in the protocol.

### 3.1 Full Labels (documentation, headers)

```
://unblock | ready       find unblocked work
://unblock | claim       take ownership
://unblock | close       complete + cascade
://unblock | depends     declare dependency
://unblock | prime       session context
://unblock | create      new issue
://unblock | stats       repository stats
://unblock | setup       configure project
```

Format: `://unblock` in brand colours, `|` pipe separator in Violet, tool name in text colour. The pipe reads like a shell command.

### 3.2 Short Labels (badges, inline references)

```
://ready    ://claim    ://close    ://depends
://stats    ://prime    ://create   ://setup
```

The product name is omitted — context makes it clear. Used in badges, tags, and inline references.

### 3.3 Badge Styles

**Outline (default):** 1px Violet border, transparent fill, Violet text. Used for inactive/available tools.

**Filled (active/highlighted):** Abyss (#1E1B4B) fill, 1px Indigo (#312E81) border, Mist (#C4B5FD) text. Used for active state or emphasis.

**Inverted (on brand background):** Violet (#A78BFA) fill, Abyss (#1E1B4B) text bold. Used for primary CTAs.

---

## 4. Colour Palette

| Name | Hex | Role |
|---|---|---|
| **Violet** | #A78BFA | Primary. The `://` mark, pipes, separators, active states |
| **Deep** | #7C3AED | `://` on light backgrounds (higher contrast) |
| **Abyss** | #1E1B4B | Filled badge backgrounds, dark violet surfaces |
| **Indigo** | #312E81 | Filled badge borders, subtle violet borders |
| **Night** | #0F172A | Primary dark background. Default context |
| **Snow** | #F8FAFC | Light background adaptation |
| **Mist** | #C4B5FD | Text on Abyss backgrounds, light violet text |

### 4.1 Desktop App Surface

The desktop app uses a deeper canvas than the general brand Night. This creates more separation between the app chrome and the brand materials.

| Token | Hex | Role |
|---|---|---|
| **Canvas** | #0D1217 | Main window background — deepest surface |
| **Surface** | #141A21 | Title bar, toolbar, panel headers |
| **Surface raised** | #1E293B | Cards, hover states, elevated elements |
| **Border subtle** | #1E293B | Panel dividers, icon containment borders |
| **Border emphasis** | #334155 | Active states, focus rings |

**Adjusted tokens on canvas #0D1217:**

| Token | Brand value | App value | Why |
|---|---|---|---|
| Filled badge bg | Abyss #1E1B4B | **#252247** | Original invisible on #0D1217 |
| Secondary text | #475569 | **#64748B** | Legibility on darker canvas |
| Icon border | none | **#1E293B** | Icons need containment on same-tone bg |

### 4.2 Status Colours (Desktop App Graph Nodes)

These are functional, not brand colours. Used only in the graph view.

| Status | Colour | Hex |
|---|---|---|
| Ready | Green | #22C55E |
| In progress | Blue | #3B82F6 |
| Blocked | Red | #EF4444 |
| Deferred | Amber | #F59E0B |
| Closed | Slate | #64748B |

---

## 5. Typography

**One font. One family. No exceptions.**

| Use | Font | Weight | Notes |
|---|---|---|---|
| `://` mark | JetBrains Mono | 700 (bold) | Always bold. The protocol mark demands visual weight |
| Wordmark | JetBrains Mono | 400 (regular) | Regular weight creates hierarchy against bold `://` |
| Tool labels | JetBrains Mono | 400 | Regular for tool names in labels |
| Pipe `\|` separator | JetBrains Mono | 400 | Same weight as surrounding text, Violet colour |
| Body text | JetBrains Mono | 400 | Documentation, descriptions, UI text |
| Emphasis | JetBrains Mono | 700 | Bold for headings and strong emphasis only |

**Fallback stack:** `'JetBrains Mono', 'Fira Code', 'SF Mono', 'Cascadia Code', monospace`

**Letter-spacing:** -0.5px on the lockup at display sizes (24px+). Default at body sizes.

---

## 6. Ecosystem

The `://` prefix is the shared mark across all websublime products. Each product has its own accent colour. The format is always: `://productname`.

| Product | Colour | Hex | Domain |
|---|---|---|---|
| ://unblock | Violet | #A78BFA | Task tracking for AI agents |
| ://line | Lime | #C8FF00 | UI components (line://ui) |
| ://beads | Green | #22C55E | AI task management CLI |
| ://cookyer | Orange | #F97316 | Kitchen management app |

---

## 7. Brand Rules

1. **All monospace** — zero sans-serif in brand materials
2. **:// always bold 700** — the protocol mark is visually heavy
3. **Pipe | as separator** — not slash, not dash, not dot. Pipe. It's a terminal
4. **Outline badges default** — filled badges only for active/highlighted state
5. **Always lowercase** — no exceptions
6. **Dark-first** — Night is the default background. Design dark, adapt light
7. **Minimum clear space** — height of the colon dots around all sides of the mark
8. **Never stretch or distort** — the monospace grid is sacred
9. **No gradients, no shadows, no decorative effects** — flat, raw, terminal

---

## 8. Voice

| Principle | Example |
|---|---|
| **Tool, not app** | "run ://ready" not "open the dashboard" |
| **Agents first** | "the agent claims #42" not "assign #42 to the agent" |
| **Graphs, not boards** | "dependencies are the primitive" not "move the card to done" |
| **Commands, not descriptions** | "://close cascades" not "the close operation triggers a cascade update" |
| **Direct** | "what can I work on?" not "let me check for available items" |

---

## 9. Asset Reference

All assets are in `assets/brand/` directory:

```
assets/brand/
├── logo/
│   ├── logo-dark.svg          # Primary lockup on dark background
│   ├── logo-light.svg         # Primary lockup on light background
│   ├── logo-violet.svg        # Primary lockup on violet background
│   └── logo-mark.svg          # :// icon mark only
├── badges/
│   ├── badge-outline.svg      # Outline badge template
│   └── badge-filled.svg       # Filled badge template
├── social/
│   └── social-card.svg        # GitHub/social preview card
└── brand-guide.md             # This document
```
