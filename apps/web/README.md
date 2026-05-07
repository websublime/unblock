# apps/web — Astro 5 + line-ui frontend

This directory will host the customer-facing web application for `://unblock`.

## Status

**Empty skeleton.** Bootstrap deferred until Stage 1 completes. Initialization
commands (when ready):

```bash
npm create astro@latest -- --template minimal
npm install @astrojs/cloudflare
```

## Planned stack

- **Framework**: Astro 5 (SSR mode on Cloudflare Pages workerd runtime)
- **Component library**: [`line://ui`](../../../vitamin) — websublime headless Web Components,
  Zag.js state machines, framework-agnostic
- **Styling**: TailwindCSS + line-ui CSS custom properties
- **Backend client**: `encore gen client --lang=typescript` generated at build time
- **BFF**: Astro Actions invoke Encore via the generated client server-side; the
  browser never touches Encore directly
- **Auth**: HttpOnly Secure cookie set on the Astro origin (`unblock.websublime.com`)
- **Live updates**: Encore Streaming (WebSocket-backed) + nanostores for shared
  island state; **no TanStack Query**

## Custom components (not provided by line-ui)

- `<DependencyGraph>` — canvas + d3-force interactive graph
- `<RoadmapTimeline>` — SVG-based Gantt timeline
- `<KanbanBoard>` — drag-and-drop via `@dnd-kit/core`
- `<MarkdownEditor>` — rich-text editing via `@tiptap/core`

See the project root `CLAUDE.md` and `docs/SPEC.md` (post-Stage-1) for the
architecture contract.
