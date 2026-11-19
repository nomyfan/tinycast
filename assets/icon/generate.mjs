// Generates Tinycast's icon SVGs: `node assets/icon/generate.mjs` (no dependencies).
// Colours can also be edited directly in the SVGs, but regenerating overwrites them:
// change them here.
import { writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const out = dirname(fileURLToPath(import.meta.url));

// ── Palette ────────────────────────────────────────────────────────────────
const C = {
  bgTop: '#2B2F34',      // background gradient, top
  bgBottom: '#121417',   // background gradient, bottom
  gloss: 0.08,           // white highlight fading out from the top (0 = none)
  bobberTop: '#FFFFFF',
  bobberBottom: '#F86044',
  line: '#FFFFFF',
  lineOpacity: 0.85,
};

// ── Geometry (1024 canvas, macOS icon grid: 824 square at 100, radius 185) ─
// The bobber sits a little below the centre so that it and the line above it
// look centred together.
const BOBBER = { cx: 512, cy: 540, r: 205, tilt: 8 };   // tilt in degrees, clockwise
const LINE = { width: 14, toY: 60 };                    // runs up along the tilt, past the top edge
const TRAY = { r: 6.6, stroke: 1.5, tilt: 15 };         // 18 pt template

// ── Helpers ────────────────────────────────────────────────────────────────
const f = n => +n.toFixed(1);
const rad = BOBBER.tilt * Math.PI / 180;
const tie = [f(BOBBER.cx + BOBBER.r * Math.sin(rad)), f(BOBBER.cy - BOBBER.r * Math.cos(rad))];
const lineEnd = [f(tie[0] + (tie[1] - LINE.toY) * Math.tan(rad)), LINE.toY];

const svg = (body, defs = []) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">\n` +
  (defs.length ? `  <defs>\n${defs.join('\n')}\n  </defs>\n` : '') +
  `${body}\n</svg>\n`;
const square = 'x="100" y="100" width="824" height="824" rx="185"';

// ── Pieces ─────────────────────────────────────────────────────────────────
const defs = {
  bg: `    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${C.bgTop}"/><stop offset="1" stop-color="${C.bgBottom}"/></linearGradient>`,
  gloss: `    <linearGradient id="gloss" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="${C.gloss}"/><stop offset=".5" stop-color="#fff" stop-opacity="0"/></linearGradient>`,
  // Light from the upper left, a shadow along the lower right edge.
  shade: `    <radialGradient id="shade" cx=".36" cy=".3" r=".8"><stop offset="0" stop-color="#fff" stop-opacity=".38"/><stop offset=".35" stop-color="#fff" stop-opacity="0"/><stop offset=".8" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity=".28"/></radialGradient>`,
  clip: `    <clipPath id="square"><rect ${square}/></clipPath>`,
};
const background = `  <rect ${square} fill="url(#bg)"/>`;
const gloss = `  <rect ${square} fill="url(#gloss)"/>`;
const line = `  <path d="M${tie.join(' ')} L ${lineEnd.join(' ')}" clip-path="url(#square)" fill="none" stroke="${C.line}" stroke-opacity="${C.lineOpacity}" stroke-width="${LINE.width}" stroke-linecap="round"/>`;
const { r } = BOBBER;
const bobber = `  <g transform="translate(${BOBBER.cx} ${BOBBER.cy}) rotate(${BOBBER.tilt})">
    <path d="M${-r} 0A${r} ${r} 0 0 1 ${r} 0Z" fill="${C.bobberTop}"/>
    <path d="M${-r} 0A${r} ${r} 0 0 0 ${r} 0Z" fill="${C.bobberBottom}"/>
    <circle r="${r}" fill="url(#shade)"/>
  </g>`;
const t = TRAY.r;

const files = {
  'app-icon.svg': svg([background, line, bobber, gloss].join('\n'), [defs.bg, defs.gloss, defs.shade, defs.clip]),
  // Separate layers for Icon Composer: the background goes in as the icon's fill.
  'layers/1-background.svg': svg(background, [defs.bg]),
  'layers/2-line.svg': svg(line, [defs.clip]),
  'layers/3-bobber.svg': svg(bobber, [defs.shade]),
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
