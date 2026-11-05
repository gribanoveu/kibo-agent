import { afterEach, describe, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ImagePart } from "../lib/images";

// Pictures pasted or dropped into the box: each goes to the backend to be
// sanitized and only what comes back is attached, sent, and drawn — and a
// model set to text only takes none.

const prepared: number[][] = [];
let refusal: string | null = null;

const part = (n: number): ImagePart => ({ mediaType: "image/png", data: `PNG${n}`, width: 10 + n, height: 5 });

const dropped: string[] = [];
/** How long the mocked backend takes to answer a pasted picture, in ms. */
let slow = 0;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    if (command === "image_prepare_file") {
      const path = (args as { path: string }).path;
      dropped.push(path);
      const n = Number(path.match(/(\d+)\.png$/)?.[1]);
      return Number.isNaN(n) ? Promise.reject("this file is not supported: save the image as PNG or JPEG") : Promise.resolve(part(n));
    }
    if (command !== "image_prepare") return Promise.resolve(null);
    const bytes = Array.from(args as Uint8Array);
    prepared.push(bytes);
    const answer = () => (refusal ? Promise.reject(refusal) : Promise.resolve(part(bytes[0])));
    return slow > 0 ? new Promise<void>((resolve) => setTimeout(resolve, slow)).then(answer) : answer();
  },
  transformCallback: (callback: unknown) => callback,
}));

// The window's own drag and drop: what the test emits is what Tauri would.
type Drop = { payload: { type: "enter" | "over" | "drop" | "leave"; paths?: string[] } };
let onDrop: ((event: Drop) => void) | null = null;
mock.module("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onDragDropEvent: (handler: (event: Drop) => void) => {
      onDrop = handler;
      return Promise.resolve(() => {
        if (onDrop === handler) onDrop = null;
      });
    },
  }),
}));
const emitDrop = (payload: Drop["payload"]) => act(() => onDrop!({ payload }));

// The platform's picture picker: what it offered, and what it answers.
let picked: string[] | null = [];
const pickerAsked: { filters?: { name: string; extensions: string[] }[] }[] = [];
mock.module("@tauri-apps/plugin-dialog", () => ({
  open: (options: { filters?: { name: string; extensions: string[] }[] }) => {
    pickerAsked.push(options);
    return Promise.resolve(picked);
  },
  save: () => Promise.resolve(null),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { Composer } = await import("../components/Composer");
const { MAX_IMAGES } = await import("../hooks/useAttachments");

afterEach(() => {
  prepared.length = 0;
  dropped.length = 0;
  pickerAsked.length = 0;
  picked = [];
  slow = 0;
  refusal = null;
  // The box keeps its text per chat, and every box here is the same chat.
  localStorage.clear();
});

/** A picture whose one byte says which it is, as the mock hands it back. */
const picture = (n: number, type = "image/png") => new File([new Uint8Array([n])], `shot${n}.png`, { type });

type Sent = { text: string; images: ImagePart[] };

function box({ imagesEnabled = true, running = false } = {}) {
  const sent: Sent[] = [];
  const queued: Sent[] = [];
  render(
    <Composer
      draftKey="draft"
      onSend={(text, images) => sent.push({ text, images })}
      onQueue={(text, images) => queued.push({ text, images })}
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
  const enter = () => fireEvent.keyDown(area, { code: "Enter", key: "Enter" });
  const altEnter = () => fireEvent.keyDown(area, { code: "Enter", key: "Enter", altKey: true });
  return { sent, queued, area, paste, type, enter, altEnter };
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

  /// Dropped anywhere on the window, the file goes to the backend by its
  /// path, and the box says it takes it while it is held over.
  test("a dropped one is taken by its path", async () => {
    box();
    await waitFor(() => expect(onDrop).not.toBeNull());
    emitDrop({ type: "enter", paths: ["/Users/me/Desktop/shot3.png"] });
    expect(screen.getByText("Drop to attach")).toBeTruthy();
    emitDrop({ type: "drop", paths: ["/Users/me/Desktop/shot3.png"] });
    expect(screen.queryByText("Drop to attach")).toBeNull();
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    expect(dropped).toEqual(["/Users/me/Desktop/shot3.png"]);
    expect(prepared).toEqual([]);
  });

  test("the drop hint goes when the drag leaves", async () => {
    box();
    await waitFor(() => expect(onDrop).not.toBeNull());
    emitDrop({ type: "over" });
    expect(screen.getByText("Drop to attach")).toBeTruthy();
    emitDrop({ type: "leave" });
    expect(screen.queryByText("Drop to attach")).toBeNull();
  });

  /// A file that is not a picture is the backend's to judge, by its content:
  /// its refusal is said by the file's name.
  test("a dropped file that is not a picture is refused by name", async () => {
    box();
    await waitFor(() => expect(onDrop).not.toBeNull());
    emitDrop({ type: "drop", paths: ["/Users/me/notes.txt"] });
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe("notes.txt: this file is not supported: save the image as PNG or JPEG"),
    );
    expect(thumbs()).toHaveLength(0);

    // Said until closed: a refusal the user has read need not stay over the box.
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("a model set to text only takes no drop either", async () => {
    box({ imagesEnabled: false });
    await waitFor(() => expect(onDrop).not.toBeNull());
    emitDrop({ type: "drop", paths: ["/Users/me/shot1.png"] });
    expect(dropped).toEqual([]);
    expect(screen.getByRole("alert").textContent).toContain("Settings → Models");
  });

  /// The box listens only while it is on screen.
  test("a box gone from the screen hears no drops", async () => {
    box();
    await waitFor(() => expect(onDrop).not.toBeNull());
    cleanup();
    await waitFor(() => expect(onDrop).toBeNull());
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
    // The ones the limit stopped before reaching leave no placeholder behind.
    expect(screen.queryAllByLabelText("Preparing a picture")).toHaveLength(0);
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

  /// A note steering a running turn takes its picture with it: a screenshot
  /// is often the whole point of the note.
  test("while a turn runs, the words steer and the picture goes with them", async () => {
    const { sent, paste, type, enter } = box({ running: true });
    paste([picture(1)]);
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    type("also check b");
    enter();
    expect(sent).toEqual([{ text: "also check b", images: [part(1)] }]);
    expect(thumbs()).toHaveLength(0);
  });

  /// Nothing typed around it: the note is the picture.
  test("a picture alone steers a running turn too", async () => {
    const { sent, paste, enter } = box({ running: true });
    paste([picture(2)]);
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    enter();
    expect(sent).toEqual([{ text: "", images: [part(2)] }]);
  });

  /// Alt+Enter holds the box for after the turn — pictures included: left in
  /// the box they would ride along with whatever is typed next.
  test("a picture held for after the turn goes with the message", async () => {
    const { sent, queued, paste, type, altEnter } = box({ running: true });
    paste([picture(3)]);
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    type("then look at this");
    altEnter();
    expect(queued).toEqual([{ text: "then look at this", images: [part(3)] }]);
    expect(sent).toEqual([]);
    expect(thumbs()).toHaveLength(0);
  });

  /// A picture being read and sanitized has a placeholder where it will land:
  /// the row does not appear out of nowhere a moment later.
  test("a picture being prepared waits as a placeholder, then becomes one", async () => {
    slow = 15;
    const { paste } = box();
    paste([picture(1), picture(2)]);
    expect(screen.getAllByLabelText("Preparing a picture")).toHaveLength(2);
    expect(thumbs()).toHaveLength(0);

    // One by one: the placeholder of the first turns into its picture while the
    // second is still being read.
    await waitFor(() => expect(thumbs()).toHaveLength(1));
    expect(screen.getAllByLabelText("Preparing a picture")).toHaveLength(1);

    await waitFor(() => expect(thumbs()).toHaveLength(2));
    expect(screen.queryAllByLabelText("Preparing a picture")).toHaveLength(0);
  });

  /// A refused picture takes its placeholder with it; the error names the file.
  test("a refused picture leaves no placeholder behind", async () => {
    slow = 15;
    refusal = "this file is not supported: save the image as PNG or JPEG";
    const { paste } = box();
    paste([picture(1)]);
    expect(screen.getAllByLabelText("Preparing a picture")).toHaveLength(1);

    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("not supported"));
    expect(screen.queryAllByLabelText("Preparing a picture")).toHaveLength(0);
    expect(thumbs()).toHaveLength(0);
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

describe("choosing a picture", () => {
  const attachButton = () => screen.queryByRole("button", { name: "Attach a picture" });

  /// The button is for a model that sees pictures, and it sits right before
  /// the send button — the two things done with a finished message.
  test("is beside the send button when the model takes pictures", () => {
    box();
    const attach = attachButton();
    expect(attach).toBeTruthy();
    expect(attach!.nextElementSibling?.getAttribute("title")).toBe("Send");
  });

  test("is not there for a model set to text only", () => {
    box({ imagesEnabled: false });
    expect(attachButton()).toBeNull();
  });

  /// PNG and JPEG are what sanitizing writes and what a provider accepts; the
  /// picker offers nothing else, and the backend refuses the rest by content.
  test("offers PNG and JPEG only, and attaches what comes back", async () => {
    picked = ["/shots/1.png"];
    box();
    await act(async () => fireEvent.click(attachButton()!));

    await waitFor(() => expect(thumbs()).toHaveLength(1));
    expect(pickerAsked[0]?.filters).toEqual([{ name: "Pictures", extensions: ["png", "jpg", "jpeg"] }]);
    expect(dropped).toEqual(["/shots/1.png"]);
    expect(thumbs()[0].getAttribute("src")).toBe("data:image/png;base64,PNG1");
  });

  test("a cancelled picker attaches nothing", async () => {
    picked = null;
    box();
    await act(async () => fireEvent.click(attachButton()!));
    expect(thumbs()).toHaveLength(0);
    expect(dropped).toEqual([]);
  });
});
