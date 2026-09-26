// `?inline`: the files arrive in the bundle as data URIs, and are decoded here
// from base64. Not fetched — the window makes no requests (the CSP refuses
// them, and so does the data-policy test), and a file read at play time would
// be heard late.
import doneUri from "../assets/task_done.mp3?inline";
import attentionUri from "../assets/need_answer.mp3?inline";

export type Sound = "done" | "attention";

const URIS: Record<Sound, string> = { done: doneUri, attention: attentionUri };

let context: AudioContext | undefined;
const buffers = new Map<Sound, Promise<AudioBuffer>>();

// Decoded once and kept: an `AudioBuffer` starts at once, where an `<audio>`
// element decodes on every play and stutters on the first.
function buffer(sound: Sound): Promise<AudioBuffer> {
  context ??= new AudioContext();
  const ctx = context;
  let decoded = buffers.get(sound);
  if (!decoded) {
    const uri = URIS[sound];
    const bytes = Uint8Array.from(atob(uri.slice(uri.indexOf(",") + 1)), (c) => c.charCodeAt(0));
    decoded = ctx.decodeAudioData(bytes.buffer);
    buffers.set(sound, decoded);
  }
  return decoded;
}

/** Decodes both sounds ahead, so the first one played is not late. */
export function preloadSounds() {
  for (const sound of Object.keys(URIS) as Sound[]) buffer(sound).catch(() => buffers.delete(sound));
}

export async function playSound(sound: Sound) {
  const decoded = await buffer(sound);
  const ctx = context;
  if (!ctx) return;
  if (ctx.state === "suspended") await ctx.resume();
  const source = ctx.createBufferSource();
  source.buffer = decoded;
  source.connect(ctx.destination);
  source.start();
}
