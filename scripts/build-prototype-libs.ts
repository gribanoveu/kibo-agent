// The libraries a prototype's page imports, built for the browser
// (docs/31-prototype-role.md, decision 2). React 19 ships CommonJS only, so it
// is bundled here once, into `react-all.js`, and each module a page names in
// its import map is a small file re-exporting from it — one React instance
// for every import, as hooks require. lucide-react is bundled with `react`
// left as an import, so its icons use that same instance. Babel and Tailwind
// are browser builds already, copied as they come.
//
// For the model's pages only — never imported by the app itself (AGENTS.md).
//
//   bun scripts/build-prototype-libs.ts [out dir]   (default: src-tauri/prototype-libs)

import { mkdirSync, copyFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";

const out = resolve(process.argv[2] ?? "src-tauri/prototype-libs");
mkdirSync(out, { recursive: true });

process.env.NODE_ENV = "production";
const require = createRequire(import.meta.url);

// What a page imports, and the name it has inside `react-all.js`.
const modules = {
  "react.js": ["react", "R"],
  "react-dom-client.js": ["react-dom/client", "D"],
  "react-jsx-runtime.js": ["react/jsx-runtime", "J"],
} as const;

// Beside this script, so `react` resolves from the project's node_modules.
const entry = join(import.meta.dir, ".prototype-libs-entry.js");
writeFileSync(
  entry,
  Object.values(modules).map(([spec, name]) => `import * as ${name} from "${spec}";`).join("\n") +
    `\nexport { ${Object.values(modules).map(([, name]) => name).join(", ")} };\n`,
);
const built = await Bun.build({
  entrypoints: [entry],
  format: "esm",
  minify: true,
  target: "browser",
  define: { "process.env.NODE_ENV": '"production"' },
}).finally(() => Bun.file(entry).delete());
if (!built.success) {
  console.error(built.logs);
  process.exit(1);
}
await Bun.write(join(out, "react-all.js"), built.outputs[0]);

for (const [file, [spec, name]] of Object.entries(modules)) {
  // A CommonJS module's names are known only by loading it.
  const names = Object.keys(require(spec)).filter((key) => key !== "default" && /^[A-Za-z_$][\w$]*$/.test(key));
  writeFileSync(
    join(out, file),
    `import { ${name} } from "./react-all.js";\n` +
      `const m = ${name}.default ?? ${name};\n` +
      `export default m;\n` +
      `export const { ${names.join(", ")} } = m;\n`,
  );
}

const lucideEntry = join(import.meta.dir, ".prototype-libs-lucide.js");
writeFileSync(lucideEntry, `export * from "lucide-react";\n`);
const lucide = await Bun.build({
  entrypoints: [lucideEntry],
  format: "esm",
  minify: true,
  target: "browser",
  external: ["react", "react/jsx-runtime"],
}).finally(() => Bun.file(lucideEntry).delete());
if (!lucide.success || lucide.outputs.length !== 1) {
  console.error(lucide.logs, lucide.outputs.map((o) => o.path));
  process.exit(1);
}
await Bun.write(join(out, "lucide-react.js"), lucide.outputs[0]);

copyFileSync(require.resolve("@babel/standalone/babel.min.js"), join(out, "babel.min.js"));
copyFileSync(require.resolve("@tailwindcss/browser"), join(out, "tailwind.js"));
console.log(`prototype libs → ${out}`);
