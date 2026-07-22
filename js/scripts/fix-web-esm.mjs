// Post-process the `dist-web` ESM build so it runs as native ES modules
// (browsers, Deno) and not only inside a bundler.
//
// `tsc` emits extensionless relative specifiers (`from "./wasm-module"`),
// which bundlers resolve but native ESM loaders reject. This rewrites every
// relative import/export in the emitted `.js`/`.d.ts` files to carry an
// explicit `.js` extension, and drops a `{"type":"module"}` marker so Node
// also treats the directory as ESM.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";

const dir = "dist-web";

// Add `.js` to a relative specifier that has no file extension yet.
const addExt = (match, pre, spec, post) =>
  /\.[a-z0-9]+$/i.test(spec) ? match : `${pre}${spec}.js${post}`;

// `from "./x"` / `from '../x'` in both `import` and `export ... from` forms.
const fromRe = /(from\s+['"])(\.\.?\/[^'"]+?)(['"])/g;

for (const file of readdirSync(dir)) {
  if (!file.endsWith(".js") && !file.endsWith(".d.ts")) continue;
  const path = `${dir}/${file}`;
  const rewritten = readFileSync(path, "utf8").replace(fromRe, addExt);
  writeFileSync(path, rewritten);
}

writeFileSync(`${dir}/package.json`, JSON.stringify({ type: "module" }));
