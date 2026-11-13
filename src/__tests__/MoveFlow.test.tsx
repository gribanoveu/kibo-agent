import { describe, expect, mock, test } from "bun:test";

// The app side of the flow "move a chat into a folder made for it": real
// hooks, real Sidebar, invoke mocked at the IPC boundary the way the backend
// answers it. The handlers below mirror App.tsx's wiring exactly.

type Call = { command: string; args: Record<string, unknown> };
const calls: Call[] = [];

let groups: { id: string; name: string; position: number }[] = [];
const chats: { id: string; title: string; updatedAt: number; archived: boolean; group?: string | null }[] = [
  { id: "c1", title: "the chat", updatedAt: 2, archived: false },
];
let nextGroup = 1;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args: args ?? {} });
    switch (command) {
      case "chat_list":
        return Promise.resolve(chats.map((c) => ({ ...c })));
      case "chat_group_list":
        return Promise.resolve(groups.map((g) => ({ ...g })));
      case "chat_group_create": {
        const made = { id: `g${nextGroup++}`, name: args!.name as string, position: groups.length + 1 };
        groups.push(made);
        return Promise.resolve(made);
      }
      case "chat_set_group": {
        const chat = chats.find((c) => c.id === args!.id);
        if (chat) chat.group = args!.group as string | null;
        return Promise.resolve();
      }
      default:
        return Promise.resolve();
    }
  },
}));
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl: async () => {} }));
mock.module("@tauri-apps/api/app", () => ({ getVersion: async () => "9.9.9-1" }));

import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { Sidebar } from "../components/Sidebar";
import { PAGES } from "../components/pages";
import { useChatHistory } from "../hooks/useChatHistory";
import { useChatGroups } from "../hooks/useChatGroups";

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

/** App.tsx's wiring, minus the rest of the app. */
function Harness({ agentMode }: { agentMode: boolean }) {
  const workspace = "/repo";
  const history = useChatHistory(agentMode ? workspace : null);
  const chatGroups = useChatGroups(agentMode ? workspace : null);
  const [, setToast] = useState<string | null>(null);
  return (
    <Sidebar
      mode="agent"
      onMode={() => {}}
      pages={PAGES}
      page={null}
      onPage={() => {}}
      chats={history.chats}
      groups={chatGroups.groups}
      groupScope={workspace}
      activeChat={null}
      onSelectChat={() => {}}
      onArchiveChat={() => {}}
      onPinChat={() => {}}
      onMoveChat={(id, group) =>
        agentMode ? history.move(id, group).catch((e) => setToast(String(e))) : void Promise.resolve()
      }
      onCreateGroup={async (name) => {
        try {
          return { id: (await chatGroups.create(name)).id };
        } catch (e) {
          return { error: String(e) };
        }
      }}
      onRenameGroup={async () => ({})}
      onDeleteGroup={() => {}}
      onDeleteChat={() => {}}
      onOpenSettings={() => {}}
      onOnboardingAction={() => {}}
      themeSide="dark"
      onToggleTheme={() => {}}
    />
  );
}

describe("move a chat into a folder made for it", () => {
  test("create from the move dialog files the chat", async () => {
    render(<Harness agentMode />);

    // The chat lists before anything happens.
    await screen.findByText("the chat");

    const row = screen.getByText("the chat").closest(".chat-row") as HTMLElement;
    fireEvent.click(within(row).getByTitle("More"));
    fireEvent.click(within(row).getByRole("menuitem", { name: "Move to folder…" }));
    const dialog = screen.getByRole("dialog", { name: /Move .+ to/ });
    fireEvent.click(within(dialog).getByTitle("Which folder holds the chat"));
    fireEvent.click(within(dialog).getByRole("option", { name: "New folder…" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "release" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => {
      const move = calls.find((c) => c.command === "chat_set_group");
      expect(move?.args).toEqual({ id: "c1", group: "g1" });
    });
  });
});
