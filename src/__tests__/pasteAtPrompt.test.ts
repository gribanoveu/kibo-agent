import { afterEach, describe, expect, test } from "bun:test";
import { Terminal } from "@xterm/xterm";
import { pasteAtPrompt, queuePaste } from "../lib/pasteAtPrompt";

// Real xterm.js screens: `sent` is what one hands its shell, and `shell`
// plays what the shell writes back to it.

type Screen = { term: Terminal; sent: () => string; shell: (text: string) => Promise<void>; stop: () => void };
const made: Screen[] = [];
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** A screen of terminal `id`; it pastes what is queued for that id. */
function screen(id: number, quietMs = 1000): Screen {
  const term = new Terminal();
  // `paste` goes through the terminal's textarea, which opening makes.
  term.open(document.body.appendChild(document.createElement("div")));
  let sent = "";
  term.onData((data) => (sent += data));
  const stop = pasteAtPrompt(term, id, quietMs);
  const made1 = {
    term,
    sent: () => sent,
    shell: (text: string) => new Promise<void>((resolve) => term.write(text, resolve)),
    stop,
  };
  made.push(made1);
  return made1;
}

afterEach(() => {
  for (const s of made.splice(0)) {
    s.stop();
    s.term.dispose();
  }
});

let nextId = 1;

describe("pasteAtPrompt", () => {
  test("at a prompt a paste goes in at once, bracketed, with no Enter after it", async () => {
    const id = nextId++;
    const s = screen(id);
    await s.shell("\x1b[?2004h% ");
    queuePaste(id, "bun test\nbun run tsc");
    expect(s.sent()).toBe("\x1b[200~bun test\rbun run tsc\x1b[201~");
  });

  test("before the prompt nothing goes in, not even one line — then it does", async () => {
    const id = nextId++;
    const s = screen(id);
    queuePaste(id, "ls -la");
    await s.shell("Last login: today\r\n");
    expect(s.sent()).toBe("");
    await s.shell("\x1b[?2004h% ");
    expect(s.sent()).toBe("\x1b[200~ls -la\x1b[201~");
  });

  test("queued before any screen is drawn, the first screen pastes it", async () => {
    const id = nextId++;
    queuePaste(id, "ls");
    const s = screen(id);
    await s.shell("\x1b[?2004h% ");
    expect(s.sent()).toBe("\x1b[200~ls\x1b[201~");
  });

  test("a screen gone before the prompt leaves the paste to the next screen of that shell", async () => {
    const id = nextId++;
    const first = screen(id);
    queuePaste(id, "ls");
    first.stop();
    await first.shell("\x1b[?2004h% ");
    expect(first.sent()).toBe("");
    const next = screen(id);
    await next.shell("\x1b[?2004h% ");
    expect(next.sent()).toBe("\x1b[200~ls\x1b[201~");
  });

  test("a screen gone takes nothing queued after it, even at a prompt", async () => {
    const id = nextId++;
    const gone = screen(id);
    await gone.shell("\x1b[?2004h% ");
    gone.stop();
    queuePaste(id, "ls");
    const next = screen(id);
    await next.shell("\x1b[?2004h% ");
    expect(gone.sent()).toBe("");
    expect(next.sent()).toBe("\x1b[200~ls\x1b[201~");
  });

  test("another shell's paste is not this one's", async () => {
    const id = nextId++;
    const s = screen(id);
    await s.shell("\x1b[?2004h% ");
    queuePaste(id + 1000, "ls");
    expect(s.sent()).toBe("");
  });

  test("a shell with no bracketed paste gets it once its output goes quiet", async () => {
    const id = nextId++;
    // Wide margins: a Windows timer runs up to 16 ms late, and more on a busy runner.
    const s = screen(id, 300);
    queuePaste(id, "dir");
    await wait(200);
    await s.shell("C:\\> ");
    // The output restarted the wait: 400 ms since the queue, 200 since the output.
    await wait(200);
    expect(s.sent()).toBe("");
    await wait(300);
    expect(s.sent()).toBe("dir");
  });

  test("queued before a screen of a shell that says nothing, it goes in after the quiet", async () => {
    const id = nextId++;
    queuePaste(id, "dir");
    const s = screen(id, 30);
    await wait(200);
    expect(s.sent()).toBe("dir");
  });

  test("a screen gone while waiting out the quiet does not paste when it ends", async () => {
    const id = nextId++;
    const gone = screen(id, 30);
    queuePaste(id, "dir");
    gone.stop();
    await wait(50);
    expect(gone.sent()).toBe("");
    const next = screen(id);
    await next.shell("\x1b[?2004h% ");
    expect(next.sent()).toBe("\x1b[200~dir\x1b[201~");
  });

  test("once pasted, later prompts and screens paste nothing again", async () => {
    const id = nextId++;
    const s = screen(id);
    await s.shell("\x1b[?2004h% ");
    queuePaste(id, "ls");
    await s.shell("\x1b[?2004l\r\nfile\r\n\x1b[?2004h% ");
    const again = screen(id);
    await again.shell("\x1b[?2004h% ");
    expect(s.sent()).toBe("\x1b[200~ls\x1b[201~");
    expect(again.sent()).toBe("");
  });
});
