// A thin pass-through to the Tauri CLI with one addition: `dev second`
// launches a second dev instance beside the first — its own identifier
// (`com.kibo.agent.dev`) and vite port, from src-tauri/tauri.dev.conf.json,
// so the single-instance lock of the first does not turn it away, and its own
// app directory (`~/.kibo-dev`) so its settings, sealed keys, chats and board
// are its own rather than the first instance's. Every other invocation
// reaches the CLI unchanged.
import { spawn } from "node:child_process";
import { homedir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const second = args[0] === "dev" && args.includes("second");
const rest = second ? args.filter((arg) => arg !== "second") : args;
const config = second ? ["--config", "src-tauri/tauri.dev.conf.json"] : [];
// What the running app reads as its own directory (`src-tauri/src/infra/app_dir.rs`).
// Already named by the caller — a directory of their own — it is left alone.
const env = second
  ? { ...process.env, KIBO_APP_DIR: process.env.KIBO_APP_DIR ?? join(homedir(), ".kibo-dev") }
  : process.env;

const child = spawn("bunx", ["tauri", ...rest, ...config], { stdio: "inherit", env });
// A bunx that is missing or cannot start is a message, not an unhandled throw.
child.on("error", (err) => {
  console.error(`cannot run bunx tauri: ${err.message}`);
  process.exit(1);
});
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => child.kill(signal));
}
// Killed by a signal there is no exit code; that is not a clean exit 0.
child.on("exit", (code, signal) => process.exit(signal ? 1 : (code ?? 1)));
