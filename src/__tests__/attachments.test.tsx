import { afterEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ImagePart } from "../lib/chat";

// Pictures pasted or dropped into the box: each goes to the backend to be
// sanitized and only what comes back is attached, sent, and drawn — and a
// model set to text only takes none.

const prepared: number[][] = [];
let refusal: string | null = null;

const part = (n: number): ImagePart => ({ mediaType: "image/png", data: `PNG${n}`, width: 10 + n, height: 5 });

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    if (command !== "image_prepare") return Promise.resolve(null);
    const bytes = Array.from(args as Uint8Array);
    prepared.push(bytes);
    return refusal ? Promise.reject(refusal) : Promise.resolve(part(bytes[0]));
  },
  transformCallback: (callback: unknown) => callback,
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { Composer } = await import("../components/Composer");
const { MAX_IMAGES } = await import("../hooks/useAttachments");

afterEach(() => {
  prepared.length = 0;
  refusal = null;
});

/** A picture whose one byte says which it is, as the mock hands it back. */
const picture = (n: number, type = "image/png") => new File([new Uint8Array([n])], `shot${n}.png`, { type });

type Sent = { text: string; images: ImagePart[] };

function box({ imagesEnabled = true, running = false } = {}) {
  const sent: Sent[] = [];
  render(
    <Composer
      onSend={(text, images) => sent.push({ text, images })}
      imagesEnabled={imagesEnabled}
      onStop={() => {}}
      running={running}
      conversation="agent"
      onConversation={() => {}}
      unattended={false}
      onUnattended={() => {}}
      models={{ choices: [], current: null }}
      onModel={() => {}}
      onEffort={() => {}}
      onLoadModels={() => {}}
      context={null}
      usage={null}
      onCompact={() => {}}
    />,
  );
  const area = screen.getByRole("textbox");
  const paste = (files: File[]) => fireEvent.paste(area, { clipboardData: { files } });
  const type = (text: string) => fireEvent.change(area, { target: { value: text } });
  const enter = () => fireEvent.keyDown(area, { key: "Enter" });
  return { sent, area, paste, type, enter };
}

const thumbs = () => screen.queryAllByRole("img");

describe("attaching a picture", () => {
  test("a pasted one is sanitized, shown, and sent with the words", async () => {
    const { sent, paste, type, enter } = box();
    // Kept out of the box: a picture is not text to paste.
    expect(paste([picture(1)])).toBe(false);
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    expect(prepared).toEqual([[1]]);
    expect(thumbs()[0].getAttribute("src")).toBe("data:image/png;base64,PNG1");

    type("what is this?");
    enter();
    expect(sent).toEqual([{ text: "what is this?", images: [part(1)] }]);
    expect(thumbs()).toHaveLength(0);
  });

  test("a picture alone is a message", async () => {
    const { sent, paste, enter } = box();
    paste([picture(2)]);
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    enter();
    expect(sent).toEqual([{ text: "", images: [part(2)] }]);
  });

  test("a dropped one is taken the same way", async () => {
    box();
    const section = screen.getByRole("textbox").closest("section")!;
    // Taken by the box, or the webview opens the file in the window.
    expect(fireEvent.dragOver(section, { dataTransfer: { types: ["Files"] } })).toBe(false);
    expect(fireEvent.dragOver(section, { dataTransfer: { types: ["text/plain"] } })).toBe(true);
    fireEvent.drop(section, { dataTransfer: { files: [picture(3)], types: ["Files"] } });
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    expect(prepared).toEqual([[3]]);
  });

  /// Pasted text is text: the box handles it as it always did.
  test("a paste without a picture is left alone", () => {
    const { paste } = box();
    const notPrevented = paste([new File(["x"], "notes.txt", { type: "text/plain" })]);
    expect(notPrevented).toBe(true);
    expect(prepared).toEqual([]);
  });

  test("a picture taken off is not sent", async () => {
    const { sent, paste, type, enter } = box();
    paste([picture(1), picture(2)]);
    await waitFor(() => expect(thumbs()).toHaveLength(2));
    fireEvent.click(screen.getByRole("button", { name: "Remove image 1" }));
    expect(thumbs()).toHaveLength(1);
    type("this one");
    enter();
    expect(sent).toEqual([{ text: "this one", images: [part(2)] }]);
  });

  test("what the backend refuses is said, by name, and not attached", async () => {
    refusal = "WebP is not supported: save the image as PNG or JPEG";
    const { paste } = box();
    paste([picture(4, "image/webp")]);
    await waitFor(() => expect(screen.getByRole("alert").textContent).toBe(`shot4.png: ${refusal}`));
    expect(thumbs()).toHaveLength(0);
  });

  /// Off for the model turns go to: nothing is sent to be prepared, and the
  /// box says where to change it.
  test("a model set to text only takes none", () => {
    const { sent, paste, type, enter } = box({ imagesEnabled: false });
    paste([picture(1)]);
    expect(prepared).toEqual([]);
    expect(screen.getByRole("alert").textContent).toContain("Settings → Models");
    type("hi");
    enter();
    expect(sent).toEqual([{ text: "hi", images: [] }]);
  });

  test(`no more than ${MAX_IMAGES} in one message`, async () => {
    const { paste } = box();
    paste(Array.from({ length: MAX_IMAGES + 2 }, (_, n) => picture(n)));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain(`Up to ${MAX_IMAGES}`));
    expect(thumbs()).toHaveLength(MAX_IMAGES);
    expect(prepared).toHaveLength(MAX_IMAGES);
  });

  /// Two pastes in quick succession share the limit: the second does not
  /// count from what the box held before the first one's pictures arrived.
  test("two pastes at once still stop at the limit", async () => {
    const { paste } = box();
    paste(Array.from({ length: 6 }, (_, n) => picture(n)));
    paste(Array.from({ length: 6 }, (_, n) => picture(n + 6)));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain(`Up to ${MAX_IMAGES}`));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(thumbs()).toHaveLength(MAX_IMAGES);
  });

  /// A note steering a running turn is text; the picture waits in the box
  /// for the next message rather than being lost or sent as a note.
  test("while a turn runs, the words steer and the picture stays", async () => {
    const { sent, paste, type, enter } = box({ running: true });
    paste([picture(1)]);
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    type("also check b");
    enter();
    expect(sent).toEqual([{ text: "also check b", images: [] }]);
    expect(thumbs()).toHaveLength(1);
    // A picture alone is not a note.
    enter();
    expect(sent).toHaveLength(1);
  });

  test("a thumbnail opens the picture whole", async () => {
    const { paste } = box();
    paste([picture(1)]);
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    await act(async () => fireEvent.click(screen.getByTitle("11×5 — click to open")));
    const dialog = screen.getByRole("dialog", { name: "Image · 11×5" });
    expect(dialog.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,PNG1");
  });
});
