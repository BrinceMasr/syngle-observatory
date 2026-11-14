// Tests of what the page-side recorder counts as an input (no browser: the script runs in a stub window).
// Run: `node --test evals/map-perf/session.test.mjs`.
import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { INIT_SCRIPT } from "./session.mjs";

/** The recorder installed in a stub window; `fire` delivers an event to the listeners of its type. */
function recorder() {
  const listeners = {};
  const window = {
    addEventListener: (type, fn) => (listeners[type] ??= []).push(fn),
    performance: { timeOrigin: 1000, now: () => 0 },
    requestAnimationFrame: () => 0,
    PerformanceObserver: class {
      observe() {}
    },
  };
  window.window = window;
  vm.runInNewContext(INIT_SCRIPT, window);
  return {
    E: window.__eval,
    fire: (type, fields) => (listeners[type] ?? []).forEach((fn) => fn({ type, timeStamp: 5, buttons: 0, ...fields })),
  };
}

test("a pointer move with no button down is not an input: a hover before a press, or the pointer's return after a release", () => {
  const { E, fire } = recorder();
  E.recording = true;
  fire("pointermove", { buttons: 0, timeStamp: 10 });
  assert.deepEqual(Array.from(E.inputs), []);
});

test("a pointer move with the button down, and a wheel step, are inputs at the event's own time", () => {
  const { E, fire } = recorder();
  E.recording = true;
  fire("pointermove", { buttons: 1, timeStamp: 20 });
  fire("wheel", { buttons: 0, timeStamp: 30 });
  assert.deepEqual(Array.from(E.inputs), [1020, 1030]);
});

test("nothing is recorded while the recorder is stopped", () => {
  const { E, fire } = recorder();
  fire("pointermove", { buttons: 1 });
  fire("wheel");
  assert.deepEqual(Array.from(E.inputs), []);
});
