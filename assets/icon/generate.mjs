// Generates Tinycast's icon SVGs: `node assets/icon/generate.mjs` (no dependencies).
// Colours can also be edited directly in the SVGs, but regenerating overwrites them:
// change them here.
import { writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { crc32, deflateSync } from 'node:zlib';

const out = dirname(fileURLToPath(import.meta.url));

// ── Palette ────────────────────────────────────────────────────────────────
const C = {
  bgTop: '#292D34',      // background gradient, top
  bgBottom: '#0E1014',   // background gradient, bottom
  gloss: 0.09,           // white highlight fading out from the top (0 = none)
  bobberTop: '#F3F5F8',
  bobberBottom: '#F86044',
  line: '#FFFFFF',
  lineOpacity: 1,
};

// ── Geometry (1024 canvas, macOS icon grid: 824 square at 100, radius 185) ─
// The bobber sits a little below the centre so that it and the line above it
// look centred together.
const BOBBER = { cx: 512, cy: 544, r: 205, tilt: 8 };   // tilt in degrees, clockwise
const LINE = { width: 10, length: 280 };                // runs up along the tilt, past the top edge
const TRAY = { r: 6.6, stroke: 1.5, tilt: 15 };         // 18 pt template

// ── Background texture ─────────────────────────────────────────────────────
// Ripples spreading from the bobber, lit from the top left, over a fine grain. Both are drawn here into
// PNGs embedded in the SVG (no <filter>, so every renderer draws them alike). Strengths are the most
// a pixel is lightened or darkened (0–1).
const RIPPLE = { strength: 0.07, wavelength: 78, fade: 300 };  // wavelength grows outward; fade: px to 1/e
const GRAIN = { strength: 0.10, size: 1.6, mottle: 0.05 };     // size: px (1 px fades out when shown smaller); mottle: ~90 px blotches

// ── Helpers ────────────────────────────────────────────────────────────────
const f = n => +n.toFixed(1);
// The line starts a little inside the sphere, which hides its end: it goes straight into the bobber.
const lineStartR = BOBBER.r - 6;

const svg = (body, defs = []) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">\n` +
  (defs.length ? `  <defs>\n${defs.join('\n')}\n  </defs>\n` : '') +
  `${body}\n</svg>\n`;
const square = 'x="100" y="100" width="824" height="824" rx="185"';
const innerSquare = 'x="101.5" y="101.5" width="821" height="821" rx="183.5"';

// A w × h grey + alpha PNG as a data URI. `value(x, y)` (pixel centres) > 0 lightens with white, < 0
// darkens with black; its size is the opacity.
function pngDataUri(w, h, value) {
  const row = 1 + w * 2, raw = Buffer.alloc(row * h);  // each row: filter byte 0, then grey/alpha pairs
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const v = Math.max(-1, Math.min(1, value(x + 0.5, y + 0.5))), i = y * row + 1 + x * 2;
    raw[i] = v > 0 ? 255 : 0;
    raw[i + 1] = Math.round(Math.abs(v) * 255);
  }
  const chunk = (type, data) => {
    const b = Buffer.alloc(12 + data.length);
    b.writeUInt32BE(data.length);
    b.write(type, 4);
    data.copy(b, 8);
    b.writeUInt32BE(crc32(b.subarray(4, 8 + data.length)), 8 + data.length);
    return b;
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;  // bit depth
  ihdr[9] = 4;  // grey + alpha
  const png = Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0)),
  ]);
  return `data:image/png;base64,${png.toString('base64')}`;
}

// Deterministic noise, so regenerating gives the same files.
const hash = (x, y) => {
  let h = Math.imul(x, 374761393) + Math.imul(y, 668265263) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return ((h ^ (h >>> 16)) >>> 0) / 2 ** 32;
};
const smoothstep = (a, b, x) => { const t = Math.min(1, Math.max(0, (x - a) / (b - a))); return t * t * (3 - 2 * t); };
// period: repeat every `period` lattice cells (for a tile), or never.
const valueNoise = (x, y, period = 0) => {
  const xi = Math.floor(x), yi = Math.floor(y), u = smoothstep(0, 1, x - xi), v = smoothstep(0, 1, y - yi);
  const at = (i, j) => period ? hash((i % period + period) % period, (j % period + period) % period) : hash(i, j);
  const a = at(xi, yi), b = at(xi + 1, yi), c = at(xi, yi + 1), d = at(xi + 1, yi + 1);
  return a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v;
};
const fbm = (x, y) => {  // 4 octaves, 0–1
  let sum = 0, amp = 0.5, freq = 1, total = 0;
  for (let i = 0; i < 4; i++, amp /= 2, freq *= 2) { sum += amp * valueNoise(x * freq + i * 17.3, y * freq - i * 9.1); total += amp; }
  return sum / total;
};

// Ripples and mottle, over the icon square only (824 px from 100, 100).
const ripples = pngDataUri(824, 824, (px, py) => {
  const x = px + 100, y = py + 100, dx = x - BOBBER.cx, dy = y - BOBBER.cy, d = Math.hypot(dx, dy);
  const mottle = (fbm(x / 90, y / 90) - 0.5) * GRAIN.mottle;
  if (d < BOBBER.r) return mottle;
  const out = d - BOBBER.r;
  const k = 2 * Math.PI / (RIPPLE.wavelength + out * 0.08);
  const fade = Math.exp(-out / RIPPLE.fade) * smoothstep(0, 25, out);
  const slope = Math.cos(out * k) * fade;      // the ripple's slope, outward
  const light = (0.6 * dx + 0.8 * dy) / d;     // > 0 where the slope faces the light (top left)
  const ruffle = (valueNoise(x / 8, y / 8) - 0.5) * 0.02 * fade;
  return slope * (RIPPLE.strength * light + 0.015) + ruffle + mottle;
});
// Grain, a 256 px tile repeated over the square (a full-size one would add a megabyte of noise).
const grainCells = Math.round(256 / GRAIN.size), grainCell = 256 / grainCells;
const grain = pngDataUri(256, 256, (x, y) => (valueNoise(x / grainCell, y / grainCell, grainCells) - 0.5) * 2 * GRAIN.strength);

// ── Pieces ─────────────────────────────────────────────────────────────────
const { r } = BOBBER;
const defs = {
  bg: `    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${C.bgTop}"/><stop offset="1" stop-color="${C.bgBottom}"/></linearGradient>`,
  bgSpot: `    <radialGradient id="bg-spot" cx=".48" cy=".42" r=".58"><stop offset="0" stop-color="#424956" stop-opacity=".58"/><stop offset=".6" stop-color="#23272E" stop-opacity=".18"/><stop offset="1" stop-color="#0E1014" stop-opacity="0"/></radialGradient>`,
  // 3D sphere shading for each hemisphere (opaque stops so SVG & PNG renderers match cleanly).
  topSphere: `    <radialGradient id="top-sphere" cx="-72" cy="-102" r="${f(r * 1.56)}" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#FFFFFF"/><stop offset=".48" stop-color="#FAFBFC"/><stop offset=".75" stop-color="#DCE2EA"/><stop offset=".90" stop-color="#B8C1CE"/><stop offset="1" stop-color="#9CA6B5"/></radialGradient>`,
  botSphere: `    <radialGradient id="bot-sphere" cx="-76" cy="-70" r="${f(r * 1.60)}" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#FF8C72"/><stop offset=".36" stop-color="#F86044"/><stop offset=".72" stop-color="#D13922"/><stop offset=".90" stop-color="#A62615"/><stop offset="1" stop-color="#8C1D0F"/></radialGradient>`,
  coralGlow: `    <radialGradient id="coral-glow" cx="526" cy="635" r="325" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#F86044" stop-opacity=".16"/><stop offset=".55" stop-color="#F86044" stop-opacity=".05"/><stop offset="1" stop-color="#F86044" stop-opacity="0"/></radialGradient>`,
  gloss: `    <linearGradient id="gloss" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="${C.gloss}"/><stop offset=".45" stop-color="#fff" stop-opacity="0"/></linearGradient>`,
  bevel: `    <linearGradient id="bevel" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".24"/><stop offset=".35" stop-color="#fff" stop-opacity=".05"/><stop offset=".85" stop-color="#fff" stop-opacity="0"/><stop offset="1" stop-color="#fff" stop-opacity=".05"/></linearGradient>`,
  lineGrad: `    <linearGradient id="line-grad" x1="${f(-LINE.width / 2)}" y1="0" x2="${f(LINE.width / 2)}" y2="0" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#DCE2EA"/><stop offset=".30" stop-color="#FFFFFF"/><stop offset=".68" stop-color="#E4E9F0"/><stop offset="1" stop-color="#9CA6B4"/></linearGradient>`,
  // Shadows behind the bobber & line (no SVG <filter>, so PNG & SVG renderers match).
  shadowBobber: `    <radialGradient id="shadow-bobber" cx=".5" cy=".5" r=".5"><stop offset="0" stop-color="#000000" stop-opacity=".72"/><stop offset=".54" stop-color="#000000" stop-opacity=".64"/><stop offset=".69" stop-color="#000000" stop-opacity=".42"/><stop offset=".80" stop-color="#000000" stop-opacity=".20"/><stop offset=".90" stop-color="#000000" stop-opacity=".06"/><stop offset="1" stop-color="#000000" stop-opacity="0"/></radialGradient>`,
  grain: `    <pattern id="grain" width="256" height="256" patternUnits="userSpaceOnUse"><image width="256" height="256" href="${grain}"/></pattern>`,
  clip: `    <clipPath id="square"><rect ${square}/></clipPath>`,
  bobberClip: `    <clipPath id="bobber-clip"><circle r="${r}"/></clipPath>`,
};

const background = [
  `  <rect ${square} fill="url(#bg)"/>`,
  `  <rect ${square} fill="url(#bg-spot)"/>`,
  `  <rect ${square} fill="url(#coral-glow)"/>`,
].join('\n');

const backgroundRich = [
  background,
  `  <image x="100" y="100" width="824" height="824" clip-path="url(#square)" href="${ripples}"/>`,
  `  <rect ${square} fill="url(#grain)"/>`,
].join('\n');

const lineShadowStrokes = [14, 12, 10, 8, 6, 4]
  .map(w => `      <line x1="0" y1="${f(-lineStartR)}" x2="0" y2="${f(-lineStartR - LINE.length)}" stroke="#000000" stroke-opacity="0.025" stroke-width="${w}" stroke-linecap="round"/>`)
  .join('\n');

const shadows = [
  `  <g clip-path="url(#square)">`,
  `    <g transform="translate(${BOBBER.cx + 5} ${BOBBER.cy + 7}) rotate(${BOBBER.tilt})">`,
  lineShadowStrokes,
  `    </g>`,
  `    <circle cx="${BOBBER.cx + 14}" cy="${BOBBER.cy + 28}" r="${r + 75}" fill="url(#shadow-bobber)"/>`,
  `  </g>`,
].join('\n');

const hw = LINE.width / 2;
const line = [
  `  <g clip-path="url(#square)">`,
  `    <g transform="translate(${BOBBER.cx} ${BOBBER.cy}) rotate(${BOBBER.tilt})">`,
  `      <path d="M${-hw} ${f(-lineStartR - LINE.length)} L${hw} ${f(-lineStartR - LINE.length)} L${hw} ${f(-lineStartR)} L${-hw} ${f(-lineStartR)} Z" fill="url(#line-grad)"/>`,
  `    </g>`,
  `  </g>`,
].join('\n');

const bobber = `  <g transform="translate(${BOBBER.cx} ${BOBBER.cy}) rotate(${BOBBER.tilt})">
    <g clip-path="url(#bobber-clip)">
      <path d="M${-r} 0A${r} ${r} 0 0 1 ${r} 0Z" fill="url(#top-sphere)"/>
      <path d="M${-r} 0A${r} ${r} 0 0 0 ${r} 0Z" fill="url(#bot-sphere)"/>
      <line x1="${-r}" y1="1.2" x2="${r}" y2="1.2" stroke="#6E150B" stroke-opacity="0.32" stroke-width="2.4"/>
      <line x1="${-r}" y1="-0.8" x2="${r}" y2="-0.8" stroke="#FFFFFF" stroke-opacity="0.55" stroke-width="1.5"/>
    </g>
  </g>`;

const tileOverlay = [
  `  <rect ${square} fill="url(#gloss)"/>`,
  `  <rect ${innerSquare} fill="none" stroke="url(#bevel)" stroke-width="3"/>`,
].join('\n');

const t = TRAY.r;

const files = {
  'app-icon.svg': svg(
    [backgroundRich, shadows, line, bobber, tileOverlay].join('\n'),
    [
      defs.bg, defs.bgSpot, defs.coralGlow, defs.grain, defs.gloss, defs.bevel,
      defs.topSphere, defs.botSphere,
      defs.lineGrad,
      defs.shadowBobber,
      defs.clip, defs.bobberClip,
    ],
  ),
  // Separate layers for Icon Composer: the background goes in as the icon's fill.
  'layers/1-background.svg': svg(backgroundRich, [defs.bg, defs.bgSpot, defs.coralGlow, defs.grain, defs.clip]),
  'layers/2-line.svg': svg(line, [defs.lineGrad, defs.clip]),
  'layers/3-bobber.svg': svg(bobber, [
    defs.topSphere, defs.botSphere,
    defs.bobberClip,
  ]),
  // Menu bar: the tilted bobber alone, an 18 pt template image (black + alpha; macOS tints it).
  'tray-icon.svg': `<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 18 18">
  <g transform="translate(9 9) rotate(${TRAY.tilt})">
    <circle r="${t}" fill="none" stroke="#000" stroke-width="${TRAY.stroke}"/>
    <path d="M${-t} 0A${t} ${t} 0 0 0 ${t} 0Z" fill="#000"/>
  </g>
</svg>\n`,
};
// Layers of the earlier fishing-rod icon.
for (const old of ['layers/2-rod.svg', 'layers/3-line-bobber.svg']) rmSync(join(out, old), { force: true });
for (const [name, content] of Object.entries(files)) {
  mkdirSync(dirname(join(out, name)), { recursive: true });
  writeFileSync(join(out, name), content);
}
console.log(Object.keys(files).join('\n'));
