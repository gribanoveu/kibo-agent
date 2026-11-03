import { afterEach, describe, expect, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import { chatDraftKey, forgetChatDraft, readChatDraft, useChatDraft } from "../hooks/useChatDraft";

// The box's text lives in this window's storage, keyed by the chat it was typed
// in: the whole point is that a key that changes brings another chat's text,
// and that the next launch finds it again.
afterEach(() => localStorage.clear());

describe("where a chat's draft is kept", () => {
  test("names the mode and the chat, and gives the chat not saved yet one slot", () => {
    expect(chatDraftKey("agent", "c1")).toBe("chat-draft.agent.c1");
    expect(chatDraftKey("chat", "c1")).toBe("chat-draft.chat.c1");
    expect(chatDraftKey("agent", null)).toBe("chat-draft.agent.new");
    // A chat id that is nothing is a chat not saved yet, not a key of its own.
    expect(chatDraftKey("agent", "")).toBe("chat-draft.agent.new");
  });
});

describe("the box's draft", () => {
  const box = (key: string) => renderHook(({ key }: { key: string }) => useChatDraft(key), { initialProps: { key } });

  test("what was typed in one chat is not in the next, and is back in its own", () => {
    const { result, rerender } = box(chatDraftKey("agent", "a"));
    act(() => result.current[1]("про первый"));

    rerender({ key: chatDraftKey("agent", "b") });
    expect(result.current[0]).toBe("");

    act(() => result.current[1]("про второй"));
    rerender({ key: chatDraftKey("agent", "a") });
    expect(result.current[0]).toBe("про первый");
    rerender({ key: chatDraftKey("agent", "b") });
    expect(result.current[0]).toBe("про второй");
  });

  test("the next launch finds it", () => {
    const first = box(chatDraftKey("agent", "a"));
    act(() => first.result.current[1]("не дописано"));
    first.unmount();

    const second = box(chatDraftKey("agent", "a"));
    expect(second.result.current[0]).toBe("не дописано");
  });

  /// Sent or cleared, the box holds nothing — and neither does the storage.
  test("an empty box leaves no row behind", () => {
    const key = chatDraftKey("agent", "a");
    const { result } = box(key);
    act(() => result.current[1]("отправлено"));
    expect(localStorage.getItem(key)).toBe("отправлено");

    act(() => result.current[1](""));
    expect(localStorage.getItem(key)).toBeNull();
    expect(result.current[0]).toBe("");
  });

  test("the chat not saved yet is one slot, so the next new chat finds the text", () => {
    const { result, rerender } = box(chatDraftKey("agent", null));
    act(() => result.current[1]("черновик нового"));

    rerender({ key: chatDraftKey("agent", "a") });
    expect(result.current[0]).toBe("");
    rerender({ key: chatDraftKey("agent", null) });
    expect(result.current[0]).toBe("черновик нового");
  });
});

describe("the chat deleted", () => {
  /// The chat's own draft goes with it; Chat mode's chat with the same id, and
  /// the chat not saved yet, stay where they are.
  test("takes its draft and no other", () => {
    const mine = chatDraftKey("agent", "a");
    const other = chatDraftKey("chat", "a");
    const fresh = chatDraftKey("agent", null);
    localStorage.setItem(mine, "1");
    localStorage.setItem(other, "2");
    localStorage.setItem(fresh, "3");

    forgetChatDraft(mine);
    expect(readChatDraft(mine)).toBe("");
    expect(readChatDraft(other)).toBe("2");
    expect(readChatDraft(fresh)).toBe("3");
  });
});
