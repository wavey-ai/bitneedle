# Bitneedle Ordinals render runtime

This directory contains a small Bitneedle record renderer for inscriptions. It
uses the recursive inscription pattern:

- a small HTML wrapper
- one shared JS runtime inscription
- one already-encoded RGB/EnCodec byte block image
- optional on-chain artwork/label assets
- JSON data that describes how to render the record
- browser PNG export from the rendered canvas.

This renderer is not Press or WASM. The browser decodes images and supplies the
canvas, text, clipping, and blending functions. The runtime maps decoded RGB
pixels to the Bitneedle spiral. It also paints a small label. The runtime parses
and validates each manifest before the wrapper starts the render operation.

## Runtime

Source: [`src/bitneedle-ordinal-runtime.js`](src/bitneedle-ordinal-runtime.js)

Global API:

```js
await BitneedleOrdinal.renderRecord(manifest, { canvas });
await BitneedleOrdinal.bootstrap();
await BitneedleOrdinal.downloadPng(renderResultOrCanvas);
const validation = BitneedleOrdinal.validateManifest(manifest);
const compact = BitneedleOrdinal.compactManifest(verboseManifest);
```

`bootstrap()` reads JSON from:

```html
<script id="bitneedle-ordinal-record" type="application/json">...</script>
```

and renders into:

```html
<canvas id="bitneedle-record" width="576" height="576"></canvas>
```

Compact manifests use the `record-core` spiral by default. The JavaScript
runtime uses the same traversal as the Rust implementation:

1. Start at the 12 o'clock position.
2. Trace clockwise from the outer payload radius.
3. Decrease the radius linearly by the pitch.
4. Use rounding that gives the same result in JavaScript and Rust.
5. Read pixels in the traced spiral order.

The old recursive logarithmic mask is not a valid Bitneedle payload geometry.
The Ordinals renderer does not use this mask.

The default label style is still `master`, which keeps the original demo/master
copy aesthetic separate from the canonical payload spiral.

## Dev server

From the repo root:

```sh
npm install
npm run server
```

Open `http://127.0.0.1:5177/`. The server:

- bundles `ordinals/src/bitneedle-ordinal-runtime.js` with esbuild
- watches the runtime and rebuilds on edit
- serves an editable JSON manifest test UI
- proxies `/content/<inscription>` to `https://ordinals.com/content/<inscription>`
- wires the rendered canvas to PNG download.

Useful flags:

```sh
npm run server -- --port 5188
npm run server -- --manifest ordinals/examples/recursive-manifest.json
```

## Compact manifest shape

The runtime accepts a compact shape for wrappers:

```json
{
  "v": 1,
  "p": "single45",
  "r": "/content/<rgb-block-inscription>",
  "a": "/content/<artwork-inscription>",
  "la": "/content/<label-artwork-inscription>",
  "bg": "#fafafa",
  "fg": "#000000",
  "bl": 96973,
  "lr": 149,
  "on": "1,852,218,650,130,935",
  "sat": "849,379",
  "bn": "71867558",
  "l": ["Title", "Artist", "Subtitle", "A", "2:53", "2024", "Runout text"]
}
```

Aliases:

- `p`: record profile (`single45`, `ten`, or `lp`)
- `r`: RGB block / track-map image
- `a`: artwork image
- `la`: label artwork image
- `bg`: page/record background
- `fg`: record color
- `sm`: spiral mode (`record-core`, the default)
- `ls`: label style (`master`, the default, or `clean`)
- `b`: explicit `record-core` spiral pitch or b-value. Omit it to calculate the
  pitch from the payload pixel count
- `bl`: original byte length, if known
- `pc`: payload pixel count, if known
- `cs`: crop source RGB image scanlines when a payload image actually includes non-payload top/bottom rows
- `lr`: label radius override
- `ao`: record artwork opacity
- `lao`: label artwork opacity
- `oo`: overlay opacity
- `g`: geometry overrides
- `on`: ordinal/runout number
- `sat`: satoshi index text
- `bn`: block height text
- `mb`: mined-by text
- `l`: label array (`title`, `artist`, `subtitle`, `side`, `duration`, `year`, `runout`, `rightText`, `leftText`)

The same data can be provided as readable JSON:

```json
{
  "record": { "profile": "single45" },
  "assets": {
    "rgbBlock": "/content/<rgb-block-inscription>",
    "artwork": "/content/<artwork-inscription>"
  },
  "render": {},
  "label": {
    "title": "On-chain Record",
    "artist": "Bitneedle",
    "side": "A",
    "duration": "2:53"
  }
}
```

Compact readable JSON locally before inscription:

```sh
node ordinals/tools/compact-manifest.mjs manifest.json > manifest.compact.json
```

Generate a recursive wrapper that references a runtime inscription:

```sh
node ordinals/tools/make-wrapper.mjs \
  --runtime /content/<runtime-inscription> \
  ordinals/examples/recursive-manifest.json > wrapper.html
```

The generated wrapper includes a `Download PNG` button wired to
`BitneedleOrdinal.downloadPng()`. The file is browser-encoded with
`canvas.toBlob("image/png")`.

## Rust spiral parity check

The parity tool compares the JS spiral index order against `record-core` by
running the Rust `spiral_summary` example and hashing the ordered pixel indices:

```sh
node ordinals/tools/compare-rust-spiral.mjs
```

`npm run ordinals:check` runs this parity check after syntax checks. This is the
correctness gate for payload placement and decode order.

## Press JSON integration

The runtime also examines `press`, `pressJson`, `ordinalPaths`, `paths`, and
`assets`. Thus, a Press API payload can contain Ordinals content paths. The
runtime converts these paths to the same internal renderer model.

Expected Press-side direction:

```json
{
  "press": {
    "recordProfile": "single45",
    "streamByteLength": 96973,
    "ordinalPaths": {
      "rgbBlock": "/content/<rgb-block-inscription>",
      "artwork": "/content/<artwork-inscription>"
    }
  }
}
```

## Current limitations

This is the first JS-only renderer pass:

- render-only
- no authoring UI
- no video-frame extraction
- no HEIC/AVIF decoders beyond browser-native image support
- no neural audio codec
- no Rust/WASM
- no BRD1/BRS1 descriptor spiral or sidecar verification yet.

The next step is to identify the Press render fields that Ordinals requires.
Then, map these fields to the compact manifest. Do not add the complete Rust
authoring interface.
