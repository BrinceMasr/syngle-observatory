// Frame times and input-to-frame latency from what the page recorded. Pure functions: every time is in epoch milliseconds
// (a frame's own timeOrigin plus its timestamp), so the browser's, the view's and this process's clocks compare directly.
import { shareOver, summarize } from "../lib/stats.mjs";

/** An interval above this is at least one missed 60 Hz frame (16.7 ms plus 3.3 ms of jitter); the "frames over 16.7 ms". */
export const MISSED_FRAME_MS = 20;
/** The same for a 30 Hz target (33.3 ms plus 3.3 ms). */
export const MISSED_30_MS = 36.7;

/** Row layout of `window.__spikeApp.frames` (apps/rustgis-web/src/web.rs `spike_sample`). Append-only: a contract. */
export const ROW = { now: 0, renders: 1, fast: 2, renderMs: 3, features: 4, visible: 5, windowed: 6, points: 7, res: 8, cx: 9, cy: 10 };

/**
 * Frame intervals (ms) between consecutive animation-frame callbacks that fall inside the gesture windows
 * (`[[from, to], ...]`, epoch ms). Intervals across a gap between two windows are not counted.
 */
export function frameIntervals(ticks, windows) {
  const out = [];
  for (const [from, to] of windows) {
    const inside = ticks.filter((t) => t >= from && t <= to);
    for (let i = 1; i < inside.length; i++) out.push(inside[i] - inside[i - 1]);
  }
  return out;
}

export function frameStats(intervals) {
  const s = summarize(intervals);
  return { ...s, over60: shareOver(intervals, MISSED_FRAME_MS), over30: shareOver(intervals, MISSED_30_MS), fps: s.mean ? Math.round(10000 / s.mean) / 10 : null };
}

const sameCamera = (a, b) => a[ROW.res] === b[ROW.res] && a[ROW.cx] === b[ROW.cx] && a[ROW.cy] === b[ROW.cy];

/**
 * Input-to-frame latency. For each input event (epoch ms) the first view frame after it whose `changed(row, previousRow)`
 * is true is the frame that carried it; the latency is from the event to the next animation-frame callback after that
 * frame, which is when the finished frame has been handed to the compositor. `samples` are rows with their epoch time
 * appended as `row.at`. Events that no frame answered are counted as `unanswered`.
 */
export function latencies({ inputs, samples, ticks, changed = (row, previous) => !sameCamera(row, previous) }) {
  const out = [];
  let unanswered = 0;
  let cursor = 1;
  for (const input of inputs) {
    while (cursor < samples.length && samples[cursor].at <= input) cursor++;
    let k = cursor;
    while (k < samples.length && !changed(samples[k], samples[k - 1])) k++;
    if (k >= samples.length) {
      unanswered++;
      continue;
    }
    const carried = samples[k].at;
    const present = ticks.find((t) => t > carried);
    if (present === undefined) {
      unanswered++;
      continue;
    }
    out.push(present - input);
  }
  return { ...summarize(out), unanswered, events: inputs.length };
}

/** What the view's own samples say over a scenario: its render cost, how many full re-renders, how many points it drew. */
export function appStats(samples) {
  if (!samples.length) return { frames: 0 };
  const first = samples[0];
  const last = samples[samples.length - 1];
  const renderMs = samples.map((r) => r[ROW.renderMs]).filter((v) => v > 0);
  const gaps = [];
  for (let i = 1; i < samples.length; i++) gaps.push(samples[i].at - samples[i - 1].at);
  return {
    frames: samples.length,
    appFrameMs: summarize(gaps),
    renderMs: summarize(renderMs),
    fullRenders: last[ROW.renders] - first[ROW.renders],
    fastRepaints: last[ROW.fast] - first[ROW.fast],
    featuresHeld: last[ROW.features],
    layersVisible: last[ROW.visible],
    windowedLayers: last[ROW.windowed],
    pointsSubmitted: last[ROW.points],
  };
}
