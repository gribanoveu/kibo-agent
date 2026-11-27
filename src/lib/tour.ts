// The tour a new user is offered after setup: which parts of the window are
// what, and where the card sits — pure, so the placing is a test of its own.

/** A step of the tour: what to ring, and what to say about it. */
export type TourStep = {
  id: string;
  /** A CSS selector for what the step points at. The window carries `data-tour` for it. */
  target: string;
  title: string;
  body: string;
};

/**
 * The tour, and only the things a first screen cannot show: what is already
 * laid out around the chat. Anything that needs a folder open is left to the
 * moment it is wanted.
 */
export const TOUR_STEPS: TourStep[] = [
  {
    id: "sidebar",
    target: '[data-tour="sidebar"]',
    title: "Your chats",
    body: "Every folder you open keeps its chats here, and the folder's name on top. ⌘B hides the list when you want the room.",
  },
  {
    id: "panes",
    target: '[data-tour="panes"]',
    title: "The panels",
    body: "This menu opens the panels of the window, each one closing again from its own header:",
  },
  {
    id: "composer",
    target: '[data-tour="composer"]',
    title: "The message box",
    body: "Ask for something here. In Agent mode the agent works in the open folder; in Chat mode it answers without one. Enter sends, Shift+Enter is a new line.",
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
