# Playground UI grammar

Rules for the browser playground (`crates/wasm/web/index.html`). Every UI
change follows this grammar. When a change needs a new pattern, add the
pattern here in the same commit.

## Information architecture

- The tab row holds two groups with a visual divider and group labels:
  - **Clients**: surfaces that speak the wire protocol the way a customer
    would (CLI, JS SDK, Raw JSON). A client tab never invents flags or
    operations that Amazon DynamoDB does not have.
  - **Tools**: form-driven surfaces (Vector Workbench). A tool composes real
    wire calls behind forms.
- The Vector Workbench holds subsections (today: Vectors). A subsection is a
  `.subsection` container. New subsections are additive siblings.
- Each subsection: a shared context bar on top (pickers used by every
  section), then collapsible sections.

## Collapsible sections

- Plain `<details class="vec-sec" data-sec="...">` + `<summary>`. No
  framework.
- The summary carries the section title plus a one-line hint
  (`.sum-hint`, muted, middot-separated). The hint is the only place for
  explanation text; the open view holds forms, not documentation.
- Open state persists in localStorage, keyed by `data-sec`.
- Default state: every section starts closed on first visit. Only the
  user opens sections; the opened state persists.

## Buttons

- Labels are verb-first: `Run`, `Search`, `Refresh`, `Create table`,
  `Embed & search`.
- No decorative glyphs in run-style buttons. No `▶`, no `↻`. The arrow
  `→` is allowed only as a transformation arrow in a label
  (`Embed → vector JSON`).
- Exactly one `.primary` (filled) button per panel or per section: its
  main action. Everything else is a default button.
- The theme toggle is the only emoji button.

## Consoles (client tabs)

- Sample buttons sit above the input, under a `samples-label`.
- Every console input runs with Ctrl/⌘+Enter, and its label says so.
- The single run button is `.primary` and labeled `Run`.
- Sample content is runnable as-is against the seeded data. Samples that
  need seeded vectors are function-valued and built from the real seed at
  click time.

## Labels and pills

- Input labels: small uppercase (`label` default styling), no trailing
  colon.
- Pills (`.pill`) are read-only status or identity (model pill, embed
  status, item count, index summary). A pill is never clickable.
- Field notes (`.field-note`): one small muted line directly under an
  input, only when a default needs explanation. Never a paragraph.

## Repeatable rows

- Optional flat attributes use repeatable name + value rows
  (`.attr-row`) with an `Add attribute` button and a per-row `Remove`.
- Value types auto-detect: a numeric string becomes `N`, `true`/`false`
  becomes `BOOL`, everything else stays `S`. No nested types, no JSON
  editor.
- Rows with an empty name are skipped on submit.

## Vectors and long values

- Never render a full vector inline. Every vector display is a chip:
  three leading dims to three decimals, ellipsis, dim count
  (`[-0.080, -0.006, 0.123, …] · 384d`), with `expand` and `copy`
  controls. Full JSON appears only on expand, in a fixed-height
  scrollable monospace box.
- Long strings in table cells truncate at 80 chars with the full value
  in `title`.

## Log pane

- Every action logs exactly one entry: one-line header (time, op, status
  pill, duration), then request/response body boxes.
- Long bodies clamp behind the `show more` control. Machine-payload
  bodies (for example wire vectors) start fully collapsed behind the same
  control.
- `logNote` is only for engine lifecycle notes (seed, reset, failures),
  never for operation output.

## Global messaging

- The in-tab promise (runs in this tab, no server, no network after the
  initial load) is stated exactly once, in the page header subtitle.
  Tab headers, seed notes, and pills do not repeat it.

## Terminology

- Operations use wire casing in prose: `PutItem`, `SearchVectors`.
  CLI command names keep kebab-case: `aws dynamodb search-vectors`.
- The one term for the request-shaped vector encoding is
  "wire vector JSON".
- The upstream service is "Amazon DynamoDB" in docs and comments.

## Seeds

- `Books` (30 rows, Author HASH + Title RANGE): the general-purpose
  non-vector demo table.
- `Quotes` (120 rows, 384-d COSINE index): the vector demo table. Seed
  embeddings are precomputed with the same pinned model the page loads.
- Disable-with-reason: when a form cannot apply to the current selection,
  disable its inputs and show a plain visible reason next to them
  (`.why`). Tooltips alone are not enough.
