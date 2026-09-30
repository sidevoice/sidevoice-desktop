# Web client brand audit (rubasace/sidevoice `apps/web`)

Audited read-only at **origin/main `7565663`** against the brand's rules (sidevoice/brand-resources@`506fcc1`
README and brand sheet), with `213790b` as the reference for how tokens were mapped. Every `file:line` below is
at `7565663`, relative to the rubasace/sidevoice root.

Then, by the operator's extension of the task, the fixes that stay inside tokens, CSS and assets were applied on
**`brand/web`** (pushed; not merged): `536fa0f`, `3be4bec`. A trial merge of `brand/web` over
`integrate/cursor-on-split` (`7b677bf`) is clean and builds; the pairing dialog of that merge is
`docs/brand-screens/web-pairing-brand-web-over-split.png`. Verified on the **built** CSS (the room serves a
build, and `react.css` beats `room.css` at equal weight): every changed rule was read back from
`dist/assets/index-*.css` with its competitors, and the page was rendered headless
(`docs/brand-screens/web-room-brand-web.png`, `web-header-brand-web.png`: DM Sans loaded, the lockup in the header).

## Status at a glance

| ID | Finding (short) | Severity | Status |
|---|---|---|---|
| L1 | Header "Sidevoice" is typed text in lila, not the lockup | high | **fixed** — the outlined `sidevoice-lockup-on-dark.svg` at 26 px (`RoomHeader.tsx`, `.brand img`) |
| T1 | DM Sans never loaded; "Inter" named but not loaded either | high | **fixed** — `DMSans-Variable-latin.woff2` + `OFL.txt` in `src/assets/brand/`, `@font-face` and `--sv-font` in `tokens.css`, `:root` uses it |
| C1 | "Enviar": noche on plum, 1.29 : 1 | high | **fixed** — soft lila text (9.2 : 1) and a visible hover, in `react.css` (kept off the line the functional branch changes) |
| M1 | Favicons 404 on the production server (`/voice/favicon.svg` not routed) | high | **fixed** — moved to `src/assets/brand/`; the build emits them under `/voice/assets/` (checked in `dist/index.html`) |
| C2 | Primary buttons turn grey on hover (1.44 : 1) | medium | **fixed** — `.ui-button--primary:hover` = `--sv-primary-soft` (11.9 : 1 with noche) |
| C3 | White on `#ea4335` danger button, 3.92 : 1 | medium | **fixed** — new `--sv-danger-fill: #c7392d` (5.18 : 1); `--sv-danger` itself unchanged (see S1) |
| F1 | Fields get the platform's blue focus ring | medium | **fixed** — lila `:focus-visible` outline on input/select/textarea |
| S3 | The call button is Meet green | medium (question) | **open — brand owner** |
| F2 | No `::selection` (system blue) | low | **fixed** — `--sv-primary-muted` ground, text colour kept (10.4 : 1) |
| F3 | No root `accent-color` | nit | **fixed** — `:root { accent-color: var(--sv-primary) }` |
| B1–B3 | Blue-tinted greys in stats headers, loading dialog, language rows | low | **fixed** — through `--sv-muted`, `--sv-text`, `--sv-border` |
| L2 | `SidevoiceMark` in any ink other than lila gives a look the kit lacks | low | not changed — the header no longer uses it; nothing else does |
| L3 | Mark at 22 px box | nit | superseded by L1 |
| S1 | `#ea4335` (Google's red) as danger | low (question) | **open — brand owner** (the brand sheet itself uses it) |
| S5 | Warning amber `#e0a640` sits on mustard's hue | low (question) | **open — brand owner** |
| M6 | Mic meter: five solid lila bars at the mark's size | low (question) | **open — brand owner** |
| S2, S4, S6 | Reds, greens and Meet greys not routed through tokens | low / nit | not changed — consistency, not a brand rule; heavy edits on lines the functional branches also touch |
| M4 | No web app manifest (brand ships 192/512 icons) | low | not changed — new server routing needed (same as M1) |
| M5 | No artwork in the call's Media Session metadata | low | not changed — lives in `room-session-controller.js`, functional code |
| T2, T3, B4, B5, O1, O2 | Dead selectors, `kbd` font, the browser-audio test page, token provenance | nit / low | not changed — cleanup, not brand |

Four decisions are the brand owner's, not this audit's: **S1** the danger red, **S3** the call button's green,
**S5** the warning amber next to mustard, **M6** the five-bar lila mic meter.

---

## Full findings (as audited at `7565663`)

## 1. Logo / mark / wordmark

| # | Where | What is wrong (rule) | Sev | Fix |
|---|---|---|---|---|
| L1 | `apps/web/src/features/room/RoomHeader.tsx:6`, `apps/web/src/styles/room.css:171` (`.brand{color:var(--sv-primary)}`), `apps/web/src/styles/room.css:1` (`h1{font-size:18px;font-weight:600}`, `:root{font-family:Inter,…}`) | The header is `<SidevoiceMark /> Sidevoice`, so "Sidevoice" is **typed text used as the logo**, which breaks "never retype the wordmark in another typeface". It renders in Inter/system-ui at 600, with no −1.5 % tracking and no opsz 40. The word is also **lila**: the kit's `sidevoice-lockup-on-dark.svg` and the brand-sheet header both draw it in white / text colour, with only the bars in lila. The mark-to-word spacing and the cap-height-to-mark ratio are also not the lockup's (lockup: text starts at 130/108 of the mark box, cap ≈ 0.67 of the box; here gap 9 px and cap ≈ 0.57). | **High** | Inline the outlined `logo/lockup/sidevoice-lockup-on-dark.svg` (viewBox `0 0 555.73 108`) at 24–26 px height (the brand sheet uses 26). Keep wordmark fill `#ffffff` or `var(--sv-text)`, bars `#c9b6ec`, fourth bar `#e0a82e`. Keep the `<h1>` accessible name: `role="img" aria-label="Sidevoice"` on the svg, or a visually hidden "Sidevoice". |
| L2 | `apps/web/src/components/ui/Icons.tsx:84-85` (`SidevoiceMark`) | Four bars use `fill="currentColor"` and the fourth is always mustard. Today it is correct only because `.brand` sets lila (`room.css:171`). In any other ink it produces a look the kit doesn't have: e.g. white bars plus full-strength mustard. The kit's one-ink version draws the fourth bar at **45 % of the ink**. | Low | Pin the colour: `fill="var(--sv-primary)"` on the four bars. Or add a `look` prop: `dark` = lila + mostaza; `ink` = currentColor + `fill-opacity=.45` on bar 4. |
| L3 | `apps/web/src/components/ui/Icons.tsx:84` (default `size = 22`) | Mark box 22 px, drawn height 19.25 px. The ≥ 20 px lockup rule is met by the box but not by the drawn bars. Clear space passes: 19 px above/below in the 60 px header, 24 px left padding, 9 px to the word; ≥ 5.5 px is needed. | Nit | If L1 is done, size the lockup at ≥ 24 px height. |
| ✓ | `Icons.tsx:85` | Geometry is exact: rects x = 1.5 / 6 / 10.5 / 15 / 19.5, y = 9 / 6 / 1.5 / 6 / 9, h = 6 / 12 / 21 / 12 / 6, w 3, rx 1.5. Mustard is on bar 4 via `--sv-voice` (#e0a82e). Lila on dark. No berenjena on dark, no lila on light. | pass | — |

## 2. Typography

| # | Where | What is wrong | Sev | Fix |
|---|---|---|---|---|
| T1 | `apps/web/src/styles/room.css:1` (`:root{font-family:Inter,system-ui,sans-serif}`) | The brand typeface is **DM Sans (OFL)**. "Inter" is declared but **never loaded**: no `@font-face`, no fontsource dependency in `apps/web/package.json`, nothing in `public/`. The whole UI therefore renders in the platform font (SF / Segoe / Roboto / Cantarell), or in Inter only if the user happens to have it installed. | **High** | Copy `brand-resources/fonts/DMSans-Variable-latin.woff2` and `OFL.txt` into `apps/web/src/assets/fonts/`. Put it under `src/` so Vite emits it under `/voice/assets/`, which the server does serve; see M1. In `tokens.css` add `@font-face{font-family:"DM Sans";src:url("../assets/fonts/DMSans-Variable-latin.woff2") format("woff2");font-weight:100 1000;font-display:swap}` and `--sv-font:"DM Sans",system-ui,sans-serif`, then use `font-family:var(--sv-font)` in `room.css:1`. The existing 550/650 weights work with the variable axis. |
| T2 | `apps/web/src/styles/room.css:1` (`kbd{font:11px system-ui}`) | Hard-coded system font. The selector is dead: no `<kbd>` is rendered. | Nit | Delete it, or use `font:11px var(--sv-font)`. |
| T3 | `packages/browser-audio/index.html:1` (`body{font:16px system-ui}`) | The standalone Kokoro test page served at `/voice-browser/` is not in DM Sans. | Low | Leave it as a dev page, or use the same `--sv-font`. |

## 3. Accent blues and blue-tinted greys

**No Google or Material blues remain.** I grepped for `#a8c7fa #8ab4f8 #1a73e8 #4285f4 #174ea6 #d2e3fc #c2e7ff #0b57d0` and for every blue that `213790b` replaced (`#b5d6ff #a8caff #b6d7ff #c6defa #192b42 #95bde6 #294b68 #304761 #dbeaff #bddbff #d4e3f4 #c9dfff #a8cbed #9fb9d2 #d5e7fb #cfe0f5 #172437 #374a63 #2f4a6b #6088b4 #233348 #dce9fb #d8e8fb`). None appear in `apps/web` or `packages/browser-audio`.

What is left are cool greys at hue ≈ 211°. The baseline is the brand sheet's own dark interface, which the brand kit takes from these tokens (`brand-resources/scripts/sheet.mjs:9`): `#14171b #1c2128 #252b33 #303742 #e8edf1`, muted `#a1acb9` (chroma 24). Anything at that chroma is a neutral. The entries below are the ones clearly bluer than that, which read as the old accent family next to lila.

| # | Where | Literal (chroma) | Sev | Fix |
|---|---|---|---|---|
| B1 | `apps/web/src/styles/room.css:101` `.stats-table th{color:#9fb4ca}` (live: ConnectionStatsDialog) | `#9fb4ca` (43): pale-blue column headers, the bluest live text colour | Low | `var(--sv-muted)` |
| B2 | `apps/web/src/styles/room.css:24` `#voice-loading{border:1px solid #657a90}`, `#voice-loading h2{color:#edf4fc}`, `#voice-loading p` and `#loading-percent{color:#b9c9da}` (live: PreparationDialog) | `#657a90` (43) steel-blue border; `#b9c9da` (33); `#edf4fc` (blue-white) | Low | `var(--sv-border)` (or a new `--sv-border-strong`); `var(--sv-text)`; `var(--sv-muted)` |
| B3 | `apps/web/src/styles/room.css:17` `.language-row-head{color:#aabccc}` (live: LanguageModelList) | `#aabccc` (34) | Low | `var(--sv-muted)` |
| B4 | Dead selectors with blue text or background: `room.css:1` `.message .who{#a2b6ca}` and `#live{background:#1b2531}`; `room.css:41` `.message-time{#a2b6ca}`; `room.css:9` `.group-label{#95a6b8}`; `room.css:37` `.person.unreachable .person-name{#9fb0c0}` | `#a2b6ca` (40), `#1b2531` (sat 29 %), `#95a6b8`, `#9fb0c0` | Nit | Delete. React renders `.chat-bubble` / `.chat-sender` / `.chat-meta time`; `#live`, `.who`, `.message-time`, `.group-label` and `.unreachable` are never rendered. |
| B5 | `packages/browser-audio/index.html:1` | `#151b23 #263342 #536578 #202a36 #a7b8ca #e8eef6`: the old slate-blue family. The page also has no favicon and no mark. | Low | Swap to the tokens' values (`#14171b/#252b33/#303742/#a1acb9/#e8edf1`), or accept it as an internal dev page. |

Judged neutral and not flagged: `#526173` (dialog, tooltip and field borders: `room.css:12,17,31,32`, `react.css:67,68,163,168`; a lighter step of `--sv-border`'s family; optionally tokenise it as `--sv-border-strong`). Field grounds `#141b23 #151c24 #10151b #1c242e #222a34 #242d37 #252d37 #29333e #202730`. Greys `#7f8b99 #7f90a3 #8d9aa8 #99a8b8 #9aa6b3 #9aa8b7 #a4b1bf #a7b4c2 #a8b5c4 #aab6c4 #afbcc9 #c9d2db #d5dde6 #e0e6ed #e3e8ee #ecf1f6`, all ≈ `--sv-muted` / `--sv-text`. Replacing them with the tokens would be a cleanup, not a brand fix.

## 4. Semantic colours (danger / positive / warning), Google palettes

| # | Where | What | Sev | Fix |
|---|---|---|---|---|
| S1 | `apps/web/src/styles/tokens.css:19` `--sv-danger:#ea4335` (used by `react.css:3` `.ui-button--danger`) | This is **Google's red**. **Open question for the brand owner:** the brand README keeps "red for danger", and `brand-resources/scripts/sheet.mjs:10` uses exactly `APP.danger:'#ea4335'` for the brand-sheet danger chip, so the brand itself currently endorses it. | Low (question) | Decide in brand-resources first; the app follows. If it changes, change `sheet.mjs` and `tokens.css` together. |
| S2 | `apps/web/src/styles/room.css:56-59` (`#mic-control[data-muted=true]{#601410}`, `#mute[aria-pressed=true]{background:#f9dedc;color:#601410}`, `:hover{#ffebe9}`) and `room.css:85` (`#connect.joined{#dc362e}`, `:hover{#ef443b}`) | Muted mic and hang-up use the **Material 3 error tonal palette** (error20 `#601410`, error90 `#f9dedc`, error50 `#dc362e`), a copy of Google Meet's look ("Meet-style", `room.css:46`). They don't use `--sv-danger`. Other untokenised reds: `#ff7272` (`room.css:26,44,122,137`), `#ffb4ab`/`#ffd4ce` (`react.css:228-229`), `#ffc7c3` (`react.css:72`), `#f0afa6` (`room.css:1,12`), `#de6253` (`room.css:26`, dead), `#e0a0a0`/`#b96a6a` (`room.css:163-164`), `#f2c6bd`/`#7d4a45`/`#2a1d1cee` (`room.css:150`). | Low | Add `--sv-danger` (fill), `--sv-danger-text` (light red for text on dark), and `--sv-danger-container` / `--sv-on-danger-container`, then route every red through them. |
| S3 | `apps/web/src/styles/room.css:13,84` `#connect{background:#18875a;color:white}`, `:hover{#229d6c}` | The page's main action (call) is a **Meet green**. It is neither lila-with-noche primary (the brand sheet's primary button) nor the brand's mint positive `#75d6ad`. Cascade: `react.css:2` `.ui-button--primary` and `room.css:1` `#connect{background:var(--sv-primary)}` are both dead for this button, because the id rules at `room.css:13` / `:84` win. | Medium (question) | Either keep call = green as a semantic and tokenise it (`--sv-call:#18875a`; white icon 4.51:1), or use mint `var(--sv-positive)` with a noche/tinta icon (9.1 / 9.6:1). If calling is to be the brand's primary, use lila + noche. |
| S4 | Positive family is untokenised: `react.css:104` `#a7d7c9`, `react.css:220` `.voice-wave{#b9f1da}`, `react.css:94` user bubble `#25423a/#2f5a4b`, `room.css:38,164` dots `#5bbf8a`, `room.css:120,136` `#64a64d` (Material-ish olive green) | Five greens beside the brand's mint `#75d6ad`, which only `#mute.holding` (`room.css:13`) uses. | Low | Route them through `--sv-positive` plus derived steps (`-soft`, `-container`). |
| S5 | Warning ambers ≈ mustard: `room.css:121,138` `#e0a640`, `room.css:34,39` `#e0b657`, `room.css:26` `#f3c563`, `react.css:95` `#f0b66c` | `#e0a640` is almost identical to mostaza `#e0a82e` (same hue, 38°). The brand reserves mustard for "the other voice", and here a warning dot wears it. | Low (question) | Add `--sv-warning` in a hue clearly apart from mostaza, e.g. an orange around `#f2994a`, or accept it and record the decision. |
| S6 | Google Meet grey family: `room.css:43,48,50,51,53,54,67,69,73,74,76,77,79,81,132`, `react.css:69,286`: `#202124 #292a2d #3c4043 #343538 #414245 #303134 #2b2d30 #3a3d41 #45474a #484a4c #242627 #e8eaed #bdc1c6 #c4c7c5 #9aa0a6` | Neutral (chroma ≤ 7), so no rule is broken. But it is Google Grey 900/800/200/400/500 copied from Meet: a second grey family next to the brand's `#14171b/#1c2128/#252b33/#303742`. | Nit | Unify on `--sv-surface*` / `--sv-border` / `--sv-muted`. |

## 5. Contrast

Computed with the WCAG formula.

| # | Where | Pair | Ratio | Sev | Fix |
|---|---|---|---|---|---|
| C1 | `apps/web/src/styles/room.css:27` `.text-composer button{background:var(--sv-primary-muted)}` vs `react.css:2` `.ui-button--primary{color:var(--sv-on-primary)}` | **"Enviar"** (`TranscriptPanel.tsx:47`, variant primary). The room.css rule (0,1,1) overrides the background but not the colour, so the label is **noche `#2a1a40` on `#3a2f52`**. This predates the rebrand (it was `#172437` on `#294b68`); `213790b` carried it over. | **1.29:1** | **High** | Delete the background override so the button is lila with noche (8.62:1). Or add `color:var(--sv-primary-soft)` (9.23:1). |
| C2 | `apps/web/src/styles/room.css:1` `button:hover{background:#333d49}` (0,1,1) beats `.ui-button--primary` (0,1,0) | On hover, **"Reintentar"** (`main.tsx:51`) and **"Recargar"** (`ErrorBoundary.tsx:34`) become noche on `#333d49`. "Guardar cambios" is safe only because `.settings-footer button` (`room.css:15`) comes later. | **1.44:1** | Medium | `react.css`: `.ui-button--primary:hover{background:color-mix(in oklab,var(--sv-primary) 85%,white)}`, or scope the room.css hover to `:where(button):hover`. |
| C3 | `apps/web/src/styles/react.css:3` `.ui-button--danger{background:var(--sv-danger);color:white}` ("Sí, revocar" / "Sí, quitar", `MachineList.tsx:49-51`, compact .78rem) | white on `#ea4335` fails AA for small text. The grey hover from C2 also replaces the red. | **3.92:1** | Medium | A darker fill such as `#c5221f` (5.80:1) or `#d93025` (4.77:1). Or the brand-sheet chip style: red text and border on transparent (4.58:1 on `#14171b`, but 4.13:1 on the aside `#1c2128`, so pick the darker fill there). |
| ✓ | — | noche on lila 8.62; lila on `#14171b` 9.75; `.chat-sender` lila on bubble `#202730` 8.17; lila on `#1c2128` 8.77; stats caption 7.54; `#speed-value` on dialog 7.86; read ticks lila on user bubble `#25423a` 5.93; `--sv-primary-dim` on `#1c2128` 7.27; `--sv-primary-soft` on `--sv-primary-muted` 9.23, on `--sv-selected` 11.2; selected border vs surface 4.10 (non-text ≥ 3); `#601410` on `#f9dedc` 10.3; white on hang-up `#dc362e` 4.56 (icon) | pass | — | — |

## 6. Focus, selection, native controls, links

| # | Where | What | Sev | Fix |
|---|---|---|---|---|
| F1 | `apps/web/src/styles/room.css:12` (`dialog select,dialog textarea`), `room.css:17` (`.language-row input`), `room.css:31` (`#pane-advanced input,#elevenlabs-key,#stt-key`), `react.css:157-168` (`.settings-dialog .model-picker` select) | **Fields have no focus style**, so they get the browser's default ring: platform blue in Chrome and Safari. Only buttons (`room.css:1`), the composer textarea (`room.css:27`), and the `summary` elements (`room.css:41,80`) get the lila ring. The settings dialog, the most field-heavy screen, shows blue rings. | Medium | `:where(input,select,textarea):focus-visible{outline:2px solid var(--sv-primary);outline-offset:1px}` in `react.css` or `tokens.css`. |
| F2 | (none defined) | There is no `::selection` rule, so selected transcript text uses the system highlight (blue on most platforms). | Low | `::selection{background:var(--sv-primary);color:var(--sv-on-primary)}` (8.6:1), or `background:var(--sv-primary-muted)` keeping the text colour (10.4:1). |
| F3 | `room.css:13` (`#tts-speed`), `room.css:24` (`#loading-progress`) | `accent-color` is lila on the only range and progress controls, so this is correct today. There is no root default, so any future checkbox, radio or range comes in system blue. No `<a>` exists; the brand sheet says links take the accent. | Nit | `:root{accent-color:var(--sv-primary)}` and `a{color:var(--sv-primary)}` as prevention. |
| ✓ | — | Placeholders use the UA grey; disabled states use opacity (neutral); scrollbars are dark via `color-scheme:dark`; the custom select arrow is neutral `#9aa6b3` (`room.css:207`). | pass | — |

## 7. Icons, meta, manifest

| # | Where | What | Sev | Fix |
|---|---|---|---|---|
| M1 | `apps/web/index.html:7-9`, `apps/web/public/*`, `apps/web/vite.config.ts` (`base:"/voice/"`), `apps/server/sidevoice/presentation.py:149-152,283-301` | **The favicons probably 404 on a production build.** This is inferred from code; I did not build or run anything. `public/` is copied to the root of `dist/`, and the built HTML points at `/voice/favicon.svg` (Vite prefixes the base; without the prefix it would be `/favicon.svg`, which 404s too). The server only routes `/voice/` (index.html), `/voice/assets/*`, `/voice/mic_capture.js` and `/voice-browser/*`. The `/voice/{path}` catch-all exists only with a dev server (`:292-297`). Result: the brand favicon works in dev and is likely missing in the served tab. | **High** (verify) | Either move the three icons to `apps/web/src/assets/brand/` and reference them relatively in `index.html`, so Vite emits hashed copies under `/voice/assets/` (already mounted). Or add a route serving the top-level files of `WEB_DIST`. Verify with `npm run build` and `GET /voice/favicon.svg`. |
| M2 | `apps/web/public/favicon.svg`, `favicon-32.png`, `apple-touch-icon.png`; `index.html:7-9` | All three are **byte-identical** to `brand-resources/favicon/`, and the link markup is exactly the README's. `favicon.svg` switches berenjena ↔ lila by colour scheme; mustard stays. | pass | — |
| M3 | `apps/web/index.html:6` `theme-color #14171b` | Equals `--sv-bg`, the ground of the brand sheet's dark interface. | pass | — |
| M4 | `apps/web/index.html` (missing) | No web app manifest, although the brand ships `icon-192.png` and `icon-512.png` "for a web app manifest" and the room is used on phones. | Low | `manifest.webmanifest` with `name:"Sidevoice"`, `theme_color`/`background_color:"#14171b"`, and icons 192/512 from `brand-resources/favicon/`, plus `<link rel="manifest">`. It has the same routing caveat as M1. |
| M5 | `apps/web/src/services/room-session-controller.js:1847` (`new MediaMetadata({title:'Sidevoice',…})`) | No `artwork`, so the OS media controls and lock screen show a generic tile during a call. | Low | `artwork:[{src:<icon-512 url>,sizes:'512x512',type:'image/png'},{src:<icon-192 url>,sizes:'192x192',type:'image/png'}]` |
| M6 | `apps/web/src/styles/room.css:44,61` (`.mic-wave i{background:var(--sv-primary)}`), `features/call/CallToolbar.tsx:25` (five `<i>`); also `react.css:211-216` `.voice-bars` (five bars in currentColor) | The mic meter is **five rounded bars in one solid lila**, about 22 px tall, i.e. the mark's hue, count and scale without the mustard bar. It can read as a defaced mark, or as graphic_eq. The brand rule ("never five bars in one solid colour") is written for the mark, not a live meter. | Low (question) | Use a different bar count (e.g. 4 or 7), or colour the meter `var(--sv-text)` / `var(--sv-positive)` instead of the mark's lila. |

## 8. Tokens and other

| # | Where | What | Sev | Fix |
|---|---|---|---|---|
| O1 | `apps/web/src/styles/tokens.css:8-17` | Brand values are copied as literals: `#c9b6ec`, `#2a1a40`, `#e0a82e` are correct. The derived steps `--sv-primary-soft #e4dbf6`, `-dim #b7a6d6`, `-muted #3a2f52`, `--sv-selected #2a2438` and `--sv-selected-border #8a76b3` are all hue 258–261° (lila is 261°), so they stay in the brand hue. The brand kit has no record of them, and `tokens.css` doesn't record which palette version it follows. | Nit | Note the brand-resources commit next to the values, or derive the steps with `color-mix(in oklab, var(--sv-primary) N%, var(--sv-bg))` so lila stays the only source. |
| O2 | Overridden or dead lila and red mappings: `room.css:1` `#connect{background:var(--sv-primary)…}` (beaten by `:13`, `:84`); `room.css:43` `#mute[aria-pressed=true]{#ea4335}` (beaten by `:58`); `room.css:9` `#a33e48`; `room.css:13` `#d64242/#ed5555` (beaten by `:85`); `room.css:43` `#3c4043` (beaten by `:53`); `room.css:26` gradient `#398565…#de6253` (`#mute::before{display:none}` at `:43`); `room.css:1` `.receipt{#c4a781}`, `.level i{#a9d7be}`, `.avatar.you` | These make it look as if the brand is applied where it isn't (e.g. S3). | Nit | Delete the dead rules. |
| ✓ | `react.css:2`, `room.css:15` (settings save), `react.css:101` (`.chat-sender` in lila), `react.css:117` (selected model card border lila), `react.css:231` (read ticks lila), `room.css:98-99,140-142` (stats accents), focus rings on buttons and summaries | These match the brand sheet: primary lila with noche, sender and accent text in lila. | pass | — |

---

## Summary

| Theme | High | Medium | Low | Nit |
|---|---|---|---|---|
| Logo / mark (L) | 1 | 0 | 1 | 1 |
| Typography (T) | 1 | 0 | 1 | 1 |
| Blues / blue greys (B) | 0 | 0 | 4 | 1 |
| Semantic colours (S) | 0 | 1 | 4 | 1 |
| Contrast (C) | 1 | 2 | 0 | 0 |
| Focus / selection (F) | 0 | 1 | 1 | 1 |
| Icons / meta (M) | 1 | 0 | 3 | 0 |
| Tokens / other (O) | 0 | 0 | 0 | 2 |
| **Total (29)** | **4** | **4** | **14** | **7** |

**The four highs:**
- **L1:** the header's "Sidevoice" is typed text in lila, in Inter/system-ui, instead of the outlined lockup.
- **T1:** DM Sans is never loaded; the "Inter" it names isn't loaded either, so the UI renders in the platform font.
- **C1:** the "Enviar" button is noche on dark plum, at 1.29:1.
- **M1:** the favicons, byte-identical to the kit, are probably not served in a production build (inferred from the routes; needs a build to confirm).

No Google or Material blues remain.

**Decisions for the brand owner:**
- `#ea4335` as the danger red (the brand sheet itself uses it).
- The Meet-green call button.
- Warning amber `#e0a640` sitting on mustard's hue.
- The five-bar lila mic meter.
