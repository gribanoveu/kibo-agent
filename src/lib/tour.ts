// The tour a new user is offered after setup: which parts of the window are
// what, and where the card sits — pure, so the placing is a test of its own.

/** A step of the tour: what to ring, what to say about it, and what to open for it. */
export type TourStep = {
  id: string;
  /** A CSS selector for what the step points at. The window carries `data-tour` for it. */
  target: string;
  title: string;
  body: string;
  /**
   * Something the step needs on screen to point at — the Catalog is a page, not
   * a corner of the chat. The tour asks for it by this id and waits for it to
   * appear; every other step is on screen already.
   */
  opens?: string;
};

/**
 * The tour, and only the things a first screen cannot show: what is already
 * laid out around the chat. Anything that needs a folder open is left to the
 * moment it is wanted.
 */
export const TOUR_STEPS: TourStep[] = [
  {
    id: "mode",
    target: '[data-tour="mode"]',
    title: "Agent and chat",
    body: "Two modes. Agent works in the folder you open: it reads the code, edits it, runs commands, and whatever MCP servers, skills and commands you add are its own. Chat is a plain conversation with the model, and needs no folder.",
  },
  {
    id: "panes",
    target: '[data-tour="panes"]',
    title: "The panels",
    body: "Everything about the folder and the work in it opens from this menu, beside the chat — and closes again from its own header.",
  },
  {
    id: "overview",
    target: '[data-tour="overview"]',
    title: "Overview",
    body: "The work at a glance: the context left in this chat, the MCP servers that are up, what is running, and the files the agent has changed.",
  },
  {
    id: "folder",
    target: '[data-tour="folder"]',
    title: "The folder",
    body: "An agent chat works in a folder, and this is where it is chosen — the folder this chat is using, with its files, its branch and its changes.",
  },
  {
    id: "model",
    target: '[data-tour="model"]',
    title: "The model, and asking first",
    body: "The model that answers, its thinking level, and the last word on this line: ask, or auto. Auto runs a whole turn without asking — edits and commands alike; ask stops at each of them until you say yes.",
  },
  {
    id: "catalog",
    target: '[data-tour="catalog-page"]',
    title: "The Catalog",
    body: "Features to switch on, MCP servers, skills, commands, runbooks and hooks — everything the agent can be given, browsed and installed in one place.",
    opens: "catalog",
  },
  {
    id: "new-chat",
    target: '[data-tour="new-chat"]',
    title: "A new chat",
    body: "Starts a chat of its own in the open folder — its own messages, its own plan, its own changes.",
  },
];

export type Box = { top: number; left: number; width: number; height: number };

/** How far the card stands from what it points at, and from the window's edges. */
const GAP = 12;

/**
 * Where the card goes: under the thing it points at, above it when the window's
 * bottom is too close, and at the bottom when neither fits — never past the
 * window's sides.
 */
export function placeCard(
  target: Box,
  card: { width: number; height: number },
  viewport: { width: number; height: number },
): { top: number; left: number } {
  const below = target.top + target.height + GAP;
  const above = target.top - GAP - card.height;
  const top =
    below + card.height <= viewport.height - GAP
      ? below
      : above >= GAP
        ? above
        : Math.max(GAP, viewport.height - card.height - GAP);
  const centred = target.left + target.width / 2 - card.width / 2;
  const left = Math.max(GAP, Math.min(centred, viewport.width - card.width - GAP));
  return { top: Math.max(GAP, top), left };
}

/** Where the card goes when there is nothing to point at: the middle of the window. */
export function placeCentred(card: { width: number; height: number }, viewport: { width: number; height: number }) {
  return {
    top: Math.max(GAP, Math.round(viewport.height / 2 - card.height / 2)),
    left: Math.max(GAP, Math.round(viewport.width / 2 - card.width / 2)),
  };
}
