import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const iconsDir = path.resolve(__dirname, "..", "src-tauri", "icons");
fs.mkdirSync(iconsDir, { recursive: true });

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) {
      c = (c >>> 1) ^ (c & 1 ? 0xedb88320 : 0);
    }
  }
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const t = Buffer.from(type, "ascii");
  const crcBuf = Buffer.concat([t, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(crcBuf), 0);
  return Buffer.concat([len, t, data, crc]);
}

function renderIconRGBA(size) {
  const pixels = Buffer.alloc(size * size * 4);
  const setPixel = (x, y, r, g, b, a = 255) => {
    if (x < 0 || y < 0 || x >= size || y >= size) return;
    const i = (y * size + x) * 4;
    const alpha = a / 255;
    const inv = 1 - alpha;
    pixels[i] = Math.round(r * alpha + pixels[i] * inv);
    pixels[i + 1] = Math.round(g * alpha + pixels[i + 1] * inv);
    pixels[i + 2] = Math.round(b * alpha + pixels[i + 2] * inv);
    pixels[i + 3] = Math.min(255, Math.round(a + pixels[i + 3] * inv));
  };

  const radius = size * 0.22;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = Math.max(radius - x, 0, x - (size - 1 - radius));
      const dy = Math.max(radius - y, 0, y - (size - 1 - radius));
      const dist = Math.hypot(dx, dy);
      if (dist <= radius + 0.5) {
        const edge = Math.max(0, Math.min(1, radius + 0.5 - dist));
        const t = (x + y) / (2 * size);
        const r = Math.round(15 + t * 12);
        const g = Math.round(23 + t * 24);
        const b = Math.round(42 + t * 38);
        setPixel(x, y, r, g, b, Math.round(255 * edge));
      }
    }
  }

  const drawSegment = (x1, y1, x2, y2, thickness, r, g, b) => {
    const minX = Math.max(0, Math.floor(Math.min(x1, x2) - thickness - 2));
    const maxX = Math.min(size - 1, Math.ceil(Math.max(x1, x2) + thickness + 2));
    const minY = Math.max(0, Math.floor(Math.min(y1, y2) - thickness - 2));
    const maxY = Math.min(size - 1, Math.ceil(Math.max(y1, y2) + thickness + 2));
    const vx = x2 - x1;
    const vy = y2 - y1;
    const lenSq = vx * vx + vy * vy || 1;
    for (let y = minY; y <= maxY; y++) {
      for (let x = minX; x <= maxX; x++) {
        const proj = Math.max(0, Math.min(1, ((x - x1) * vx + (y - y1) * vy) / lenSq));
        const px = x1 + proj * vx;
        const py = y1 + proj * vy;
        const d = Math.hypot(x - px, y - py);
        if (d <= thickness + 0.8) {
          const alpha = Math.max(0, Math.min(1, thickness + 0.8 - d));
          setPixel(x, y, r, g, b, Math.round(alpha * 255));
        }
      }
    }
  };

  const drawCircle = (cx, cy, rad, r, g, b) => {
    const minX = Math.max(0, Math.floor(cx - rad - 2));
    const maxX = Math.min(size - 1, Math.ceil(cx + rad + 2));
    const minY = Math.max(0, Math.floor(cy - rad - 2));
    const maxY = Math.min(size - 1, Math.ceil(cy + rad + 2));
    for (let y = minY; y <= maxY; y++) {
      for (let x = minX; x <= maxX; x++) {
        const d = Math.hypot(x - cx, y - cy);
        if (d <= rad + 0.7) {
          const alpha = Math.max(0, Math.min(1, rad + 0.7 - d));
          setPixel(x, y, r, g, b, Math.round(alpha * 255));
        }
      }
    }
  };

  const n1 = [size * 0.25, size * 0.64];
  const n2 = [size * 0.5, size * 0.3];
  const n3 = [size * 0.75, size * 0.64];
  const stroke = Math.max(1.5, size * 0.055);
  drawSegment(n1[0], n1[1], n2[0], n2[1], stroke, 56, 189, 248);
  drawSegment(n2[0], n2[1], n3[0], n3[1], stroke, 56, 189, 248);
  drawSegment(n1[0], n1[1], n3[0], n3[1], stroke * 0.75, 148, 163, 184);

  const nodeR = Math.max(3, size * 0.125);
  drawCircle(n1[0], n1[1], nodeR, 248, 250, 252);
  drawCircle(n1[0], n1[1], nodeR * 0.62, 14, 165, 233);
  drawCircle(n2[0], n2[1], nodeR, 248, 250, 252);
  drawCircle(n2[0], n2[1], nodeR * 0.62, 34, 197, 94);
  drawCircle(n3[0], n3[1], nodeR, 248, 250, 252);
  drawCircle(n3[0], n3[1], nodeR * 0.62, 245, 158, 11);

  return pixels;
}

function encodePng(size, rgba) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;

  const stride = size * 4;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0;
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  const idat = zlib.deflateSync(raw, { level: 9 });
  return Buffer.concat([
    sig,
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", idat),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function encodeIco(pngBuffers) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(pngBuffers.length, 4);

  const entries = [];
  let offset = 6 + 16 * pngBuffers.length;
  for (const { size, data } of pngBuffers) {
    const entry = Buffer.alloc(16);
    entry[0] = size >= 256 ? 0 : size;
    entry[1] = size >= 256 ? 0 : size;
    entry[2] = 0;
    entry[3] = 0;
    entry.writeUInt16LE(1, 4);
    entry.writeUInt16LE(32, 6);
    entry.writeUInt32LE(data.length, 8);
    entry.writeUInt32LE(offset, 12);
    entries.push(entry);
    offset += data.length;
  }
  return Buffer.concat([header, ...entries, ...pngBuffers.map((item) => item.data)]);
}

const sizes = [32, 128];
const pngs = sizes.map((size) => {
  const data = encodePng(size, renderIconRGBA(size));
  fs.writeFileSync(path.join(iconsDir, `${size}x${size}.png`), data);
  return { size, data };
});
fs.writeFileSync(path.join(iconsDir, "icon.png"), pngs[1].data);
fs.writeFileSync(path.join(iconsDir, "icon.ico"), encodeIco(pngs));
console.log("[gen-icons] Generated icons in", iconsDir);
