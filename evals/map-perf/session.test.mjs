// Tests of what the harness sends and what the page-side recorder counts as an input (no browser: the script runs in a stub
// window). Run: `node --test evals/map-perf/session.test.mjs`. Probed in Chrome 157 on 14 Nov 2026: a move sent with `button:
// "left"` and no button down reached the page as a pointermove with `buttons` 1, so the hover before a press and the pointer's
// return after a release looked like drag moves.
import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { INIT_SCRIPT, mouseParams } from "./session.mjs";

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
    fire: (type, fields) => (listeners[type] ?? []).forEach((fn) => fn({ type, timeStamp: 5, buttons: 0, movementX: 0, movementY: 0, ...fields })),
  };
}

test("a pointer move with no button down is not an input: a hover before a press, or the pointer's return after a release", () => {
  const { E, fire } = recorder();
  E.recording = true;
  fire("pointermove", { buttons: 0, movementX: -420, timeStamp: 10 });
  assert.deepEqual(Array.from(E.inputs), []);
});

test("a move that moves the pointer by nothing is not an input, even with the button down", () => {
  const { E, fire } = recorder();
  E.recording = true;
  fire("pointermove", { buttons: 1, movementX: 0, movementY: 0, timeStamp: 10 });
  assert.deepEqual(Array.from(E.inputs), []);
});

test("a pointer move that moves with the button down, and a wheel step, are inputs at the event's own time", () => {
  const { E, fire } = recorder();
  E.recording = true;
  fire("pointermove", { buttons: 1, movementX: 8, timeStamp: 20 });
  fire("pointermove", { buttons: 1, movementY: -3, timeStamp: 25 });
  fire("wheel", { buttons: 0, timeStamp: 30 });
  assert.deepEqual(Array.from(E.inputs), [1020, 1025, 1030]);
});

test("nothing is recorded while the recorder is stopped", () => {
  const { E, fire } = recorder();
  fire("pointermove", { buttons: 1 });
  fire("wheel");
  assert.deepEqual(Array.from(E.inputs), []);
});

test("a move with no button down is sent as button none, as a mouse sends it; a press, a release and a drag move say left", () => {
  assert.equal(mouseParams("mouseMoved", 640, 400, { buttons: 0 }).button, "none");
  assert.equal(mouseParams("mouseMoved", 640, 400).button, "none");
  assert.equal(mouseParams("mouseMoved", 700, 400, { buttons: 1 }).button, "left");
  assert.equal(mouseParams("mousePressed", 640, 400, { buttons: 1 }).button, "left");
  assert.equal(mouseParams("mouseReleased", 700, 400, { buttons: 0 }).button, "left");
  assert.deepEqual(mouseParams("mouseMoved", 7, 9, { buttons: 1 }), { type: "mouseMoved", x: 7, y: 9, button: "left", clickCount: 1, buttons: 1 });
});
