// Emits the 1024x1024 source icon that `tauri icon` expands into the platform
// icon set. Kept dependency-free so the artwork is reproducible from the repo.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const SIZE = 1024;
const SAMPLES = 3; // supersampling factor per axis
const BRAND = [0x4d, 0x6b, 0xfe];
const INK = [0xff, 0xff, 0xff];

/** Signed distance to a rounded rectangle, negative inside. */
function sdRoundRect(px, py, cx, cy, halfW, halfH, radius) {
  const qx = Math.abs(px - cx) - (halfW - radius);
  const qy = Math.abs(py - cy) - (halfH - radius);
  const ax = Math.max(qx, 0);
  const ay = Math.max(qy, 0);
  return Math.hypot(ax, ay) + Math.min(Math.max(qx, qy), 0) - radius;
}

/** Signed distance to the capsule with the given half-width. */
function sdCapsule(px, py, ax, ay, bx, by, halfWidth) {
  const pax = px - ax;
  const pay = py - ay;
  const bax = bx - ax;
  const bay = by - ay;
  const h = Math.min(1, Math.max(0, (pax * bax + pay * bay) / (bax * bax + bay * bay)));
  return Math.hypot(pax - bax * h, pay - bay * h) - halfWidth;
}

/** The `>_` prompt glyph, drawn as strokes and one bar. */
function glyphDistance(x, y) {
  const STROKE = 41;
  const chevronTop = sdCapsule(x, y, 352, 372, 486, 512, STROKE);
  const chevronBottom = sdCapsule(x, y, 486, 512, 352, 652, STROKE);
  // The underscore is a rounded bar, expressed as a very short capsule.
  const bar = sdCapsule(x, y, 546, 662, 706, 662, 34);
  return Math.min(chevronTop, chevronBottom, bar);
}

const pixels = Buffer.alloc(SIZE * SIZE * 4);

for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    let bgHits = 0;
    let inkHits = 0;
    for (let sy = 0; sy < SAMPLES; sy++) {
      for (let sx = 0; sx < SAMPLES; sx++) {
        const px = x + (sx + 0.5) / SAMPLES;
        const py = y + (sy + 0.5) / SAMPLES;
        if (sdRoundRect(px, py, 512, 512, 512, 512, 224) <= 0) bgHits++;
        if (glyphDistance(px, py) <= 0) inkHits++;
      }
    }
    const total = SAMPLES * SAMPLES;
    const bgAlpha = bgHits / total;
    const inkAlpha = inkHits / total;
    // Composite the glyph over the plate, then the plate over transparency.
    const mix = (channel) =>
      Math.round(BRAND[channel] * (1 - inkAlpha) + INK[channel] * inkAlpha);
    const offset = (y * SIZE + x) * 4;
    pixels[offset] = mix(0);
    pixels[offset + 1] = mix(1);
    pixels[offset + 2] = mix(2);
    pixels[offset + 3] = Math.round(bgAlpha * 255);
  }
}

// Raw scanlines, each prefixed with a zero filter byte.
const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
for (let y = 0; y < SIZE; y++) {
  raw[y * (SIZE * 4 + 1)] = 0;
  pixels.copy(raw, y * (SIZE * 4 + 1) + 1, y * SIZE * 4, (y + 1) * SIZE * 4);
}

const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  return table;
})();

function crc32(buffer) {
  let c = -1;
  for (const byte of buffer) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

const target = process.argv[2] ?? "src-tauri/icons/source.png";
writeFileSync(target, png);
console.log(`wrote ${target} (${SIZE}x${SIZE}, ${(png.length / 1024).toFixed(1)} KiB)`);
