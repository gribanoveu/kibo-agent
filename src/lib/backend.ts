

// Whether the desktop app's backend is there to call.

/** Whether the desktop app is around: a plain browser (`bun run dev`) has no backend. */
export const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Outside the app there is no backend. Say so, rather than let `invoke` fail with its own wording. */
export function requireBackend() {
  if (!inTauri()) throw new Error("This needs the desktop app — there is no backend in a plain browser.");
}
