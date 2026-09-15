# Panel UI

Glass contract for the Waveshare 2.8" face and surfaces. Config keys that
change colour or strip occupancy live in [`CONFIG.md`](CONFIG.md). Token
words are ADR-0020. Numbers below are what the renderer paints.

240×320 portrait / 320×240 landscape. RGB565. Stock mono faces only.

---

## Themes

`theme=ink|dusk|studio|night`. Live key: toml + unit restart. Not a CSS
engine. Each name is a fixed six-token set. The Volumio plugin keeps
these keys on a **UI** section, separate from Panel (rotation, bus).

| token | paints |
|---|---|
| `bg` | ground, letterbox, IP-ring backing |
| `title` | track title, play/pause, seek fill + knob, volume +/− and readout, unmuted speaker, mode ON, status IP |
| `meta` | artist, clocks, skips, list + speaker dock, IP ring, stream/status facts |
| `dim` | album, troughs, hairlines, headers, hold countdown, mode OFF, version line |
| `accent` | volume fill only |
| `danger` | speaker + cross when muted or volume 0 |

Cover art is never recoloured. Ink's six CSS constants do not move. Night
is amber type on a near-black ground — do not brighten `bg`. Mode-ON on
Controls is the one pulled exception: ON uses `accent` (orange on Ink).

---

## Type

| face | stock font | where |
|---|---|---|
| title | `FONT_9X15_BOLD` | resting-face title; volume readout |
| meta | `FONT_6X10` | face clocks, headers, IP letters, landscape artist/album |
| surface | `FONT_10X20` | Status and Metadata body — leftover height, both orientations |

`FONT_10X20` is the largest stock face. IPv6 wraps; nothing is 2× scaled.
`status_text_*=large` still applies to the **boot** overlay only.

Portrait face shows **title only** (two lines, 40 px). Artist and album
live on the Metadata surface. Landscape still paints title / artist / album
in the 120 px column.

Status / Metadata line caps come from leftover body height (same weights
both ways). Portrait is taller, so it keeps more title lines.

---

## Spacing

Hairlines are 1 px in `dim`.

| measure | px | note |
|---|---|---|
| portrait art → title | 10 | |
| portrait title block | 40 | 2 × 15 + gap + pad |
| dock pad | 15 | divider → glyph **and** glyph → seek band |
| dock cell | 80×52 / 40×44 | portrait / landscape |
| dock glyph | 22 | speaker is the signed-off 12×10 at 2×; play is a triangle or two bars |
| seek strip | 32 / 40 | portrait / landscape; whole strip is the hit |
| seek chrome | top of strip | portrait face only — so the trough is not a second gap |
| IP ring | ø18 | 2 px in from the top-right: portrait `(220, 2)`, landscape `(300, 2)` |
| IP hit | 48×48 / 44×44 | A.1 / A.2; paint is not a 48 px slab |
| surface pad x | 12 | |
| field gap | 8 | metadata / status blocks |

---

## Portrait face (240×320)

| box | x, y, w, h |
|---|---|
| art | 27, 0, 186, 186 |
| title | 12, 196, 216, 40 |
| IP hit | 192, 0, 48, 48 |
| dock | 0, 236, 240, 52 |
| seek | 0, 288, 240, 32 |

Art is the leftover square after title + dock + seek, capped at 188 so it
stays off the ring. `strip=off` hides seek and grows that square (still
capped).

---

## Landscape face (320×240)

A.2. Art 200×200. Text column 200, 0, 120, 156 (pad 10; title under the
IP hit). Dock 200, 156, 120, 44 (cells 40×44). Seek 0, 200, 320, 40.

---

## Surfaces

One at a time. Close on 10 s or an outside tap. The remaining seconds
(`10s` … `1s`) sit in the IP box so the clock is off the body. Artwork
has no footer line.

| tap | opens |
|---|---|
| play | Controls |
| speaker | Volume |
| list / title | Metadata |
| IP ring | Status |
| cover | Artwork |

Artwork **contains** to the nearest frame edge (scale until the first edge
is hit, then letterbox). The face stays fit-in-the-square. The tap bitmap
is the face decode scaled up — it can look a little soft.

Volume is a vertical stepper: equal +/−, 44 px slider hit, 12 px trough,
accent fill. Controls: seek 40 px, SHUF / REP / ONE equal thirds; ONE is
a lamp (tap is Repeat).

---

## Neighbours that must not move

Token map and identity (except the recorded mode-ON accent). `getState`
is the only state source. No ALSA OUT. Live `theme` path. Ink CSS
constants. Dock 80×52 / 40×44 hits. Seek whole-strip hit. IP hit boxes.
Landscape A.2 column.
