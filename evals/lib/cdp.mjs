// A minimal Chrome DevTools Protocol client on Node's global WebSocket (Node 22 or newer): no dependencies, so an
// evaluator runs from a fresh checkout. Method names and parameters are the protocol's own
// (https://chromedevtools.github.io/devtools-protocol/); Orca's trace benchmarks and the upstream map's navigate.mjs use the same calls.

export class Cdp {
  #ws;
  #next = 1;
  #pending = new Map();
  #listeners = new Map();

  static connect(url, timeoutMs = 15000) {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      const timer = setTimeout(() => reject(new Error(`CDP connect timed out: ${url}`)), timeoutMs);
      ws.addEventListener("open", () => {
        clearTimeout(timer);
        resolve(new Cdp(ws));
      });
      ws.addEventListener("error", () => {
        clearTimeout(timer);
        reject(new Error(`CDP connect failed: ${url}`));
      });
    });
  }

  constructor(ws) {
    this.#ws = ws;
    ws.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      if (message.id !== undefined) {
        const waiter = this.#pending.get(message.id);
        if (!waiter) return;
        this.#pending.delete(message.id);
        if (message.error) waiter.reject(new Error(`${waiter.method}: ${message.error.message}`));
        else waiter.resolve(message.result);
        return;
      }
      for (const handler of this.#listeners.get(message.method) ?? []) handler(message.params ?? {}, message.sessionId);
    });
    ws.addEventListener("close", () => {
      for (const waiter of this.#pending.values()) waiter.reject(new Error("CDP connection closed"));
      this.#pending.clear();
    });
  }

  /** Sends one command, to the browser or (with `sessionId`) to an attached target. */
  send(method, params = {}, sessionId = undefined, timeoutMs = 60000) {
    const id = this.#next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`${method} timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.#pending.set(id, {
        method,
        resolve: (value) => (clearTimeout(timer), resolve(value)),
        reject: (error) => (clearTimeout(timer), reject(error)),
      });
      const payload = { id, method, params };
      if (sessionId) payload.sessionId = sessionId;
      this.#ws.send(JSON.stringify(payload));
    });
  }

  on(method, handler) {
    const list = this.#listeners.get(method) ?? [];
    list.push(handler);
    this.#listeners.set(method, list);
  }

  close() {
    try {
      this.#ws.close();
    } catch {
      // Already closed.
    }
  }
}
