// Numbers every evaluator shares: percentiles, medians, the noise of a score over several runs, and the keep rule of the
// hill climb (Syngle-Ecosystem #581 and #593 use the same method: a step is kept only when the median of the runs beats
// the measured noise and every gate passes).

/** Nearest-rank percentile of a list (p in 0..100). Empty list gives null. */
export function percentile(values, p) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[rank];
}

export const median = (values) => percentile(values, 50);

/** Plain summary of a list of durations or intervals, rounded to 0.1 ms where it is a time. */
export function summarize(values) {
  const clean = values.filter((v) => typeof v === "number" && Number.isFinite(v));
  if (!clean.length) return { n: 0, min: null, p50: null, p95: null, p99: null, max: null, mean: null };
  const round = (v) => Math.round(v * 10) / 10;
  return {
    n: clean.length,
    min: round(Math.min(...clean)),
    p50: round(percentile(clean, 50)),
    p95: round(percentile(clean, 95)),
    p99: round(percentile(clean, 99)),
    max: round(Math.max(...clean)),
    mean: round(clean.reduce((a, b) => a + b, 0) / clean.length),
  };
}

/** Share (0..100, one decimal) of values above a limit. */
export function shareOver(values, limit) {
  if (!values.length) return null;
  return Math.round((values.filter((v) => v > limit).length / values.length) * 1000) / 10;
}

/**
 * The noise of one score over its runs: the median, the range, and the spread (max minus min). A step must beat the
 * spread of the baseline's runs to count as a change; the spread is what repeated runs of unchanged code really did.
 */
export function noiseOf(runs) {
  const clean = runs.filter((v) => typeof v === "number" && Number.isFinite(v));
  if (!clean.length) return { runs: 0, median: null, min: null, max: null, spread: null };
  return { runs: clean.length, median: median(clean), min: Math.min(...clean), max: Math.max(...clean), spread: Math.max(...clean) - Math.min(...clean) };
}

/**
 * The keep rule. `direction` is "lower" (times, shares of bad frames) or "higher" (points on screen). A candidate is kept
 * when its median improves on the baseline's median by more than the larger of the two spreads, and `gatesPass`.
 * Returns `{ keep, delta, noise, reason }`; a step that does not clearly improve is reverted, never "kept anyway".
 */
export function decide({ baseline, candidate, direction, gatesPass }) {
  const noise = Math.max(baseline.spread ?? 0, candidate.spread ?? 0);
  const gain = direction === "lower" ? baseline.median - candidate.median : candidate.median - baseline.median;
  if (!gatesPass) return { keep: false, delta: gain, noise, reason: "a gate failed" };
  if (candidate.runs < 3) return { keep: false, delta: gain, noise, reason: "fewer than 3 runs: no measured noise" };
  if (!(gain > noise)) return { keep: false, delta: gain, noise, reason: gain > 0 ? "gain is inside the noise" : "no gain" };
  return { keep: true, delta: gain, noise, reason: "median beats the noise and the gates pass" };
}
