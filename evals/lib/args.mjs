// `--name value` and `--flag` arguments, with numbers and booleans read as such. No dependencies.
export function parseArgs(argv, defaults = {}) {
  const out = { ...defaults, _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) {
      out._.push(a);
      continue;
    }
    const [key, inline] = a.slice(2).split(/=(.*)/s, 2);
    const next = argv[i + 1];
    let value = inline;
    if (value === undefined) {
      if (next === undefined || next.startsWith("--")) value = true;
      else (value = next), i++;
    }
    if (typeof value === "string") {
      if (/^-?\d+(\.\d+)?$/.test(value)) value = Number(value);
      else if (value === "true") value = true;
      else if (value === "false") value = false;
    }
    out[key] = value;
  }
  return out;
}

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
