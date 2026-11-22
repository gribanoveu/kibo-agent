import { useCallback, useState } from "react";
import type { ImagePart } from "../lib/images";
import { COMMIT_REQUEST, type OfferKind, PULL_REQUEST, REVIEW_REQUEST } from "../lib/offer";
import { exportChat, moveChatToWorktree } from "../lib/savedChats";
import { pickSavePath } from "../lib/dialog";
import { withLanguageReminder, type ReplyLanguage } from "../lib/settings";
import type { ConversationMode } from "../lib/chat";
import { STAY } from "./useSidebarActions";
import type { useAgentTurn } from "./useAgentTurn";
import type { useChatHistory } from "./useChatHistory";
import type { useFolderSwitch } from "./useFolderSwitch";
import type { usePlainChat } from "./usePlainChat";
import type { useWorkspace } from "./useWorkspace";

/** What "Implement in Agent mode" says on the user's behalf. Shown in the transcript like anything they type. */
const IMPLEMENT_PLAN = "Implement the plan above. Work through the checklist in order.";

/**
 * Getting a chat on screen and a message out of the box: picking a chat (of
 * another folder too), starting a new one, sending, the offers after an
 * answer, folding the history, and writing the conversation out as a file.
 */
export function useChatRouting({
  agentMode,
  agent,
  plain,
  history,
  workspace,
  folderSwitch,
  conversation,
  replyLanguage,
  notify,
  focusComposer,
  leavePage,
  giveBack,
}: {
  agentMode: boolean;
  agent: Pick<ReturnType<typeof useAgentTurn>, "chatId" | "turn" | "open" | "reset" | "send" | "compact">;
  plain: Pick<ReturnType<typeof usePlainChat>, "open" | "newChat">;
  history: Pick<ReturnType<typeof useChatHistory>, "chats" | "folderOf">;
  workspace: Pick<ReturnType<typeof useWorkspace>, "path" | "open" | "pick">;
  folderSwitch: Pick<ReturnType<typeof useFolderSwitch>, "guard">;
  conversation: { value: ConversationMode; pick: (mode: ConversationMode) => Promise<string | null> };
  replyLanguage: ReplyLanguage;
  notify: (msg: string) => void;
  focusComposer: () => void;
  leavePage: () => void;
  giveBack: (text: string, images?: ImagePart[]) => void;
}) {
  // A chat to open once its folder is the active one: one moved to its
  // worktree, or one of another folder picked in the sidebar. Opened only
  // then, or the folder change's reset of the thread would close it again.
  const [pendingChat, setPendingChat] = useState<{ id: string; path: string } | null>(null);
  const pendingChatOpened = useCallback(() => setPendingChat(null), []);

  // A chat of another folder makes that folder the active one first (F-8.6):
  // the right panel and the next new chat follow the chat on screen.
  const selectChat = (id: string, folder: string | null = agentMode ? history.folderOf(id) : null) => {
    leavePage();
    if (agentMode && folder && folder !== workspace.path) {
      return void folderSwitch.guard(() => {
        setPendingChat({ id, path: folder });
        // A failed open drops only its own chat, not one picked since.
        void workspace.open(folder).then((opened) => opened || setPendingChat((pending) => (pending?.id === id ? null : pending)));
      });
    }
    // A chat of another folder still on its way is not opened over this one,
    // picked since.
    setPendingChat(null);
    void (agentMode ? agent.open(id) : plain.open(id)).then((opened) => opened || notify(STAY));
  };

  // The conversation just left is already on disk and stays in the sidebar;
  // this only stops pointing at it.
  const newChat = () => {
    leavePage();
    if (!agentMode) {
      if (!plain.newChat()) return notify(STAY);
      return focusComposer();
    }
    if (!agent.reset()) return notify(STAY);
    focusComposer();
  };

  // The section's "+": in the active folder an ordinary new chat; in another
  // one the folder becomes active, and its change starts the thread afresh.
  const newChatIn = (folder: string) => {
    setPendingChat(null);
    if (folder === workspace.path) return newChat();
    leavePage();
    void folderSwitch.guard(() => void workspace.open(folder));
  };

  // The window the meter is drawn against.
  const busy = agent.turn.status === "running" || agent.turn.status === "awaitingApproval";
  const compactNow = async () => {
    if (busy) return notify("Not while a turn is running");
    const folded = await agent.compact(true);
    // The reason comes back from the call: the hook's error is a render behind.
    if (folded !== true) notify(typeof folded === "string" ? folded : "Nothing worth folding away yet");
  };

  // Asking before the first message rather than refusing it — and then sending
  // it: the composer has already cleared the box, so anything not sent here is
  // typed twice.
  // `command`, when given, is what the model gets in place of `text` — a `/`
  // command's prompt — with the reply language said at its end.
  const send = async (text: string, command?: string, images: ImagePart[] = []) => {
    const sent = command === undefined ? undefined : withLanguageReminder(command, replyLanguage);
    // The composer has already cleared the box: a picker closed without a
    // folder gives the message back rather than losing it.
    if (!workspace.path && !(await workspace.pick())) return giveBack(text, images);
    agent.send(text, sent, images);
  };

  // The mode first, and only then the message: the backend reads the mode
  // when the turn starts, and a turn sent a moment early would still be a
  // plan that cannot write.
  const implement = async () => {
    const failed = await conversation.pick("agent");
    if (failed) return notify(failed);
    agent.send(IMPLEMENT_PLAN, undefined, [], true);
  };

  const pickOffer = async (kind: OfferKind) => {
    if (kind === "review") return send(REVIEW_REQUEST);
    if (kind === "commit") return send(COMMIT_REQUEST);
    if (kind === "pullRequest") return send(PULL_REQUEST);
    // Not `send`: that would drop the checklist the plan came with.
    if (kind === "implement") return implement();
    const id = agent.chatId;
    if (!id) return;
    try {
      const path = await moveChatToWorktree(id);
      setPendingChat({ id, path });
      // The worktree opens beside the folder: the neighbouring chat keeps its
      // running processes, terminals and checkout.
      if (!(await workspace.open(path))) {
        setPendingChat(null);
      }
      // The lists re-read as the worktree joins the open folders.
    } catch (e) {
      setPendingChat(null);
      notify(String(e));
    }
  };

  // The conversation as a file, for reading it somewhere else. Only what is
  // on disk can be written out — a turn saves when it comes to rest, so this
  // exports everything up to the one still running.
  const exportOpenChat = async () => {
    const chat = history.chats.find((one) => one.id === agent.chatId);
    if (!chat) return notify("Nothing saved to export yet");
    const path = await pickSavePath(chat.title, "md");
    if (!path) return;
    try {
      await exportChat(chat.id, path);
      notify(`Exported to ${path.split("/").pop()}`);
    } catch (e) {
      notify(String(e));
    }
  };

  return { pendingChat, pendingChatOpened, selectChat, newChat, newChatIn, send, pickOffer, implement, exportOpenChat, compactNow };
}