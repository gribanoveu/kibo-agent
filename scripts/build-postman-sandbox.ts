// Postman's own `pm`, for a collection's scripts (docs/45-http-client.md, decision 8):
// `postman-sandbox`'s browser bootcode, uvm's bridge and flatted, out of their packages
// into `src-tauri/postman-sandbox/`, which `infra/postman_sandbox.rs` embeds. The files
// are kept in Git: `cargo test` runs scripts, and needs no `bun` before it. Run this only
// to change the version — the packages are devDependencies, reached from here alone.
//
//   bun scripts/build-postman-sandbox.ts

import { copyFileSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const out = resolve(import.meta.dir, "../src-tauri/postman-sandbox");
mkdirSync(out, { recursive: true });

const folder = (name: string) => dirname(require.resolve(`${name}/package.json`));
const sandbox = folder("postman-sandbox");

// The prebuilt bootcode, out of its `module.exports = c => c(null, "...")` wrapper.
const bootcode = await new Promise<string>((done, failed) =>
  require(join(sandbox, ".cache/bootcode.browser.js"))((e: unknown, code: string) => (e ? failed(e) : done(code))),
);
writeFileSync(join(out, "bootcode.js"), bootcode);
writeFileSync(join(out, "bridge-client.js"), require(join(folder("uvm"), "lib/bridge-client.js"))());
copyFileSync(join(folder("flatted"), "min.js"), join(out, "flatted.js"));

for (const name of ["postman-sandbox", "uvm", "flatted"]) {
  const license = readdirSync(folder(name)).find((f) => /^licen[sc]e/i.test(f));
  if (!license) throw new Error(`${name} has no license file`);
  copyFileSync(join(folder(name), license), join(out, `LICENSE-${name}`));
}
