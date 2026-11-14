// Tests of the frame and latency maths with synthetic frame series (no browser). Run: `node --test evals/map-perf/frames.test.mjs`.
// The first test is the regression of the 14 Nov 2026 off-by-one: `latencies` used to start its scan at the second row, so
// the frame that answered an event inside the first frame of a window was skipped and the event was scored against the
// next event's frame, half a second later.
import assert from "node:assert/strict";
import test from "node:test";
import { appStats, latencies, ROW, windowRows } from "./frames.mjs";

/** A view sample as the page records it (`ROW` layout) with its epoch time appended as `at`. */
function row(at, fields = {}) {
  const r = new Array(Object.keys(ROW).length).fill(0);
  for (const [name, value] of Object.entries(fields)) r[ROW[name]] = value;
  return Object.assign(r, { at });
}

/** Animation-frame callbacks every `step` ms from `from` to `to`. */
const rafs = (from, to, step = 2) => Array.from({ length: Math.floor((to - from) / step) + 1 }, (_, i) => from + i * step);

const visibleChanged = (r, previous) => r[ROW.visible] !== previous[ROW.visible];

/** The toggle scenario in miniature: hide at 1000, show at 1500; each is carried by the first view frame after it. */
function toggleSeries() {
  return {
    inputs: [1000, 1500],
    before: row(400, { visible: 2 }),
    samples: [row(1002, { visible: 1 }), row(1010, { visible: 1 }), row(1502, { visible: 2 }), row(1510, { visible: 2 })],
    ticks: rafs(1000, 1600, 2),
    changed: visibleChanged,
  };
}

test("an event answered in the first frame of the window scores that frame, not the next event's", () => {
  const s = latencies(toggleSeries());
  assert.equal(s.unanswered, 0);
  assert.equal(s.n, 2);
  // Input at 1000, carried by the frame at 1002, presented at the next callback after it (1004): 4 ms. The old scan read 504.
  assert.equal(s.min, 4);
  assert.equal(s.max, 4);
});

test("the first frame is judged against the row before the window", () => {
  const series = toggleSeries();
  // Same series, but the row before the window already shows the hidden state: the first frame carries nothing, so the
  // hide waits for the next change (the show at 1502, presented at 1504) and the show is still answered in 4 ms.
  const unchanged = latencies({ ...series, before: row(400, { visible: 1 }) });
  assert.equal(unchanged.n, 2);
  assert.equal(unchanged.min, 4);
  assert.equal(unchanged.max, 504);
});

test("a frame at or before the input never answers it", () => {
  const s = latencies({
    inputs: [1000],
    before: row(900, { visible: 2 }),
    samples: [row(990, { visible: 1 }), row(1030, { visible: 1 }), row(1040, { visible: 2 })],
    ticks: rafs(900, 1100, 2),
    changed: visibleChanged,
  });
  // The change at 990 predates the input, so the answer is the next change (1040, presented at 1042).
  assert.equal(s.n, 1);
  assert.equal(s.max, 42);
});

test("an event no frame answered is counted as unanswered", () => {
  const s = latencies({ inputs: [1000], before: row(900, { visible: 2 }), samples: [row(1002, { visible: 2 })], ticks: rafs(900, 1100), changed: visibleChanged });
  assert.deepEqual([s.n, s.unanswered, s.events], [0, 1, 1]);
});

test("a camera change in the first frame of a window is answered by it (the default `changed`)", () => {
  const s = latencies({
    inputs: [2000, 2040],
    before: row(1500, { res: 10, cx: 1, cy: 1 }),
    samples: [row(2003, { res: 9, cx: 1, cy: 1 }), row(2043, { res: 8, cx: 1, cy: 1 })],
    ticks: rafs(1500, 2100, 3),
  });
  assert.equal(s.n, 2);
  assert.ok(s.max <= 6, `p95 ${s.p95}`);
});

test("the row before the window must be given, even as null", () => {
  const { before, ...without } = toggleSeries();
  assert.throws(() => latencies(without), /before/);
  assert.doesNotThrow(() => latencies({ ...without, before: null }));
  assert.equal(before.at, 400);
});

test("with nothing before the window the first row is only a baseline", () => {
  const series = toggleSeries();
  const s = latencies({ ...series, before: null });
  assert.equal(s.n, 2);
  // Event 0 has no previous row to differ from, so it waits for the next change: the show at 1502 (presented at 1504).
  assert.equal(s.max, 504);
});

test("render counters are counted from the row before the window", () => {
  const before = row(400, { renders: 10, fast: 3 });
  const inside = [row(1002, { renders: 11, fast: 3 }), row(1010, { renders: 11, fast: 4 }), row(1502, { renders: 12, fast: 4 })];
  const stats = appStats(inside, before);
  // Two full re-renders happened in the window (11 and 12), one of them in its first frame.
  assert.equal(stats.fullRenders, 2);
  assert.equal(stats.fastRepaints, 1);
  assert.equal(stats.frames, 3);
});

test("windowRows keeps the rows inside the windows and the row just before the first of them", () => {
  const all = [row(50), row(95), row(100), row(120), row(400), row(410)];
  const inWindow = (t) => (t >= 100 && t <= 150) || (t >= 400 && t <= 500);
  const { rows, before } = windowRows(all, row(10), inWindow);
  assert.deepEqual(rows.map((r) => r.at), [100, 120, 400, 410]);
  assert.equal(before.at, 95);
  // No row inside any window: nothing to count, the given row stays the baseline.
  assert.deepEqual(windowRows([row(1)], row(0), () => false), { rows: [], before: row(0) });
});
