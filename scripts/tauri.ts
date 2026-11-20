// A thin pass-through to the Tauri CLI with one addition: `dev second`
// launches a second dev instance beside the first — its own identifier
// (`com.kibo.agent.dev`) and vite port, from src-tauri/tauri.dev.conf.json,
// so the single-instance lock of the first does not turn it away. The data
// in ~/.kibo stays shared. Every other invocation reaches the CLI unchanged.
import { spawn } from "node:child_process";

const args = process.argv.slice(2);
const second = args[0] === "dev" && args.includes("second");
const rest = second ? args.filter((arg) => arg !== "second") : args;
const config = second ? ["--config", "src-tauri/tauri.dev.conf.json"] : [];

const child = spawn("bunx", ["tauri", ...rest, ...config], { stdio: "inherit" });
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
