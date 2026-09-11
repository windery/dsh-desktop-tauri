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

/** Signed distance to a capsule with the given half-width. */
function sdCapsule(px, py, ax, ay, bx, by, halfWidth) {
  const pax = px - ax;
  const pay = py - ay;
  const bax = bx - ax;
  const bay = by - ay;
  const h = Math.min(1, Math.max(0, (pax * bax + pay * bay) / (bax * bax + bay * bay)));
  return Math.hypot(pax - bax * h, pay - bay * h) - halfWidth;
}

/** Signed distance to a circle. */
function sdCircle(px, py, cx, cy, radius) {
  return Math.hypot(px - cx, py - cy) - radius;
}

/**
 * Signed distance to an axis-aligned ellipse. Exact along both axes and a close
 * approximation between them, which the supersampling below hides.
 */
function sdEllipse(px, py, cx, cy, rx, ry) {
  return (Math.hypot((px - cx) / rx, (py - cy) / ry) - 1) * Math.min(rx, ry);
}

/** The same ellipse, rotated by `angle` radians about its own centre. */
function sdEllipseRot(px, py, cx, cy, rx, ry, angle) {
  const cos = Math.cos(-angle);
  const sin = Math.sin(-angle);
  const dx = px - cx;
  const dy = py - cy;
  const lx = dx * cos - dy * sin;
  const ly = dx * sin + dy * cos;
  return (Math.hypot(lx / rx, ly / ry) - 1) * Math.min(rx, ry);
}

/**
 * Fit of the drawing to the plate.
 *
 * The whale is drawn in its own convenient coordinates, which leaves it sitting
 * high (its own centre is near y=462) and a little small for an app icon. These
 * map that drawing onto the plate so it lands centred and fills it the way a
 * macOS icon wants — scaled, not merely translated, so the proportions hold.
 */
const GLYPH_SCALE = 1.05;
const GLYPH_ORIGIN_Y = 462;

/**
 * Polynomial smooth minimum. Where a plain `Math.min` union leaves the seam
 * between two solids visible as a crease — which is exactly what made an early
 * whale look like two circles stacked — this blends them with a fillet of
 * radius about `k`, so the body reads as one organic mass.
 */
function smin(a, b, k) {
  const h = Math.max(k - Math.abs(a - b), 0) / k;
  return Math.min(a, b) - h * h * k * 0.25;
}

/**
 * A whale, assembled from rounded solids and blended into one silhouette.
 *
 * Deliberately an original mark rather than a redraw of anyone's logo: the blue
 * plate carries the brand association, and the whale only has to read as one.
 *
 * The blend radii are the whole trick. A large one fuses the head and rear
 * ovals into a single tapering body with no waist; a small one grows the flukes
 * and fin out of that body like limbs while keeping the notch between the two
 * flukes sharp, since that notch is what says "whale tail" rather than "ears".
 * The pectoral flipper is deliberately short and swept back — a long one
 * hanging below the body reads as a leg and makes the whale look stood up.
 *
 * The eye is punched back out to the plate colour — a hole, not a dark dot, so
 * it stays crisp down to 32px.
 */
function glyphDistance(px, py) {
  const DEG = Math.PI / 180;
  // Fuses the body's own parts; large enough to erase the seam between them.
  const BODY = 130;
  // Grows appendages out of the body; small enough to keep the tail's notch.
  const LIMB = 58;

  const x = 512 + (px - 512) / GLYPH_SCALE;
  const y = 512 + (py - GLYPH_ORIGIN_Y) / GLYPH_SCALE;

  const head = sdEllipse(x, y, 358, 430, 245, 196);
  const rear = sdEllipse(x, y, 588, 432, 168, 140);
  let whale = smin(head, rear, BODY);

  const stalk = sdCapsule(x, y, 618, 432, 714, 432, 70);
  whale = smin(whale, stalk, LIMB);

  const flukeUp = sdEllipseRot(x, y, 810, 350, 112, 74, -40 * DEG);
  const flukeDown = sdEllipseRot(x, y, 810, 514, 112, 74, 40 * DEG);
  // Blend the two flukes to each other only lightly, so the notch survives.
  whale = smin(whale, smin(flukeUp, flukeDown, 26), LIMB);

  const fin = sdEllipseRot(x, y, 470, 600, 80, 38, 45 * DEG);
  whale = smin(whale, fin, LIMB);

  const eye = sdCircle(x, y, 268, 380, 30);
  return Math.max(whale, -eye);
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
