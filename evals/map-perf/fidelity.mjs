// The fidelity gate of the max-points score: "N points on screen" counts only when the points are on the screen.
//
// A build could hold its frame time down by drawing fewer points than the file has (a window of the nearest 30,000, a
// sample). So the score also takes two screenshots of the same view, the layer shown and hidden. The pixels that differ are
// the layer's ink. Every point of the file that falls inside the view is projected to a pixel, and the gate is the share of
// those points with ink within `RADIUS` pixels (`recall`). A dense cluster hides its inner points under its neighbours' ink,
// which is how it looks to a person too; a point of the sparse outback that was dropped has no ink near it and counts as lost.
// Drawing every point as a marker, a cluster blob or a density surface all pass; dropping points does not.
import zlib from "node:zlib";

export const RADIUS = 3;
export const RECALL_BAR = 0.98;
const MAX_POINTS_CHECKED = 60000;

/** Decodes a PNG (8-bit, RGB or RGBA, not interlaced: what Chrome's screenshot gives) to `{ width, height, data }` RGBA. */
export function decodePng(buffer) {
  const bytes = Buffer.from(buffer);
  if (bytes.readUInt32BE(0) !== 0x89504e47) throw new Error("not a PNG");
  let at = 8;
  let width = 0;
  let height = 0;
  let colorType = 0;
  const parts = [];
  while (at < bytes.length) {
    const length = bytes.readUInt32BE(at);
    const type = bytes.toString("latin1", at + 4, at + 8);
    const body = bytes.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      if (body[8] !== 8 || body[12] !== 0) throw new Error("PNG must be 8-bit and not interlaced");
      colorType = body[9];
    } else if (type === "IDAT") parts.push(body);
    else if (type === "IEND") break;
    at += 12 + length;
  }
  if (colorType !== 6 && colorType !== 2) throw new Error(`PNG colour type ${colorType} is not RGB or RGBA`);
  const channels = colorType === 6 ? 4 : 3;
  const raw = zlib.inflateSync(Buffer.concat(parts));
  const stride = width * channels;
  const out = new Uint8Array(width * height * 4);
  let previous = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = new Uint8Array(raw.buffer, raw.byteOffset + y * (stride + 1) + 1, stride).slice();
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? line[x - channels] : 0;
      const b = previous[x];
      const c = x >= channels ? previous[x - channels] : 0;
      let add = 0;
      if (filter === 1) add = a;
      else if (filter === 2) add = b;
      else if (filter === 3) add = (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        add = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      line[x] = (line[x] + add) & 255;
    }
    for (let x = 0; x < width; x++) {
      const o = (y * width + x) * 4;
      out[o] = line[x * channels];
      out[o + 1] = line[x * channels + 1];
      out[o + 2] = line[x * channels + 2];
      out[o + 3] = channels === 4 ? line[x * channels + 3] : 255;
    }
    previous = line;
  }
  return { width, height, data: out };
}

/** 1 where the two screenshots differ by more than a few levels in any channel: the layer's ink. */
export function inkMask(shown, hidden, threshold = 24) {
  if (shown.width !== hidden.width || shown.height !== hidden.height) throw new Error("screenshots differ in size");
  const mask = new Uint8Array(shown.width * shown.height);
  let ink = 0;
  for (let i = 0; i < mask.length; i++) {
    const o = i * 4;
    const d = Math.max(Math.abs(shown.data[o] - hidden.data[o]), Math.abs(shown.data[o + 1] - hidden.data[o + 1]), Math.abs(shown.data[o + 2] - hidden.data[o + 2]));
    if (d > threshold) {
      mask[i] = 1;
      ink++;
    }
  }
  return { width: shown.width, height: shown.height, mask, ink };
}

const R = 6378137;
const mercator = (lonDeg, latDeg) => [(R * lonDeg * Math.PI) / 180, R * Math.log(Math.tan(Math.PI / 4 + (latDeg * Math.PI) / 360))];

/**
 * Share of the file's points inside the view that have ink within RADIUS pixels. `xy` is the generator's Float32 lon, lat
 * pairs; `camera` is `{ cx, cy, res }` (Web Mercator metres and metres per CSS pixel) as the view itself reported it in its
 * last frame sample; `dpr` is the screenshot's pixels per CSS pixel.
 */
export function pointRecall({ ink, xy, camera, dpr = 1 }) {
  const { width, height, mask } = ink;
  const integral = new Int32Array((width + 1) * (height + 1));
  for (let y = 0; y < height; y++) {
    let row = 0;
    for (let x = 0; x < width; x++) {
      row += mask[y * width + x];
      integral[(y + 1) * (width + 1) + x + 1] = integral[y * (width + 1) + x + 1] + row;
    }
  }
  const box = (x0, y0, x1, y1) => {
    const ax = Math.max(0, x0);
    const ay = Math.max(0, y0);
    const bx = Math.min(width, x1 + 1);
    const by = Math.min(height, y1 + 1);
    if (bx <= ax || by <= ay) return 0;
    return integral[by * (width + 1) + bx] - integral[ay * (width + 1) + bx] - integral[by * (width + 1) + ax] + integral[ay * (width + 1) + ax];
  };
  const total = xy.length / 2;
  const stepBy = Math.max(1, Math.ceil(total / MAX_POINTS_CHECKED));
  let inView = 0;
  let seen = 0;
  for (let i = 0; i < total; i += stepBy) {
    const [mx, my] = mercator(xy[i * 2], xy[i * 2 + 1]);
    const px = Math.round(((mx - camera.cx) / camera.res + width / dpr / 2) * dpr);
    const py = Math.round(((camera.cy - my) / camera.res + height / dpr / 2) * dpr);
    if (px < 0 || py < 0 || px >= width || py >= height) continue;
    inView++;
    if (box(px - RADIUS, py - RADIUS, px + RADIUS, py + RADIUS) > 0) seen++;
  }
  return { inView, seen, recall: inView ? seen / inView : null, sampledEvery: stepBy, inkPixels: ink.ink };
}

/** Reads the generator's `.xy` file (Float32 little-endian pairs) as a Float32Array. */
export function readXy(buffer) {
  const copy = new Uint8Array(buffer).slice();
  return new Float32Array(copy.buffer, 0, Math.floor(copy.byteLength / 8) * 2);
}
