/**
 * The document a scene is drawn in (`docs/55-frost-scenes.md`, decisions 1–3):
 * the app's shell round a scene's body — HTML and CSS, the app's own or the
 * model's. The frame it goes into runs no script; this policy, first in the
 * document before anything of the scene's, keeps its styles and pictures off
 * the network — a second policy the scene writes can only narrow it. What a
 * policy does not cover (`<meta http-equiv="refresh">`, `<link>`) the check of
 * a model's scene refuses (decision 8). Under reduced motion a scene stands
 * still where it is, as its card shows it: stopped outright, one built on
 * negative delays would show its start, Snowfall's flakes all above the top.
 */
export const SCENE_CSP = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:";

/** On the document's root while the scene should stand still. */
export const PAUSED = "paused";

const SHELL_STYLE =
  "html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#0b1018}" +
  `html.${PAUSED} *,html.${PAUSED} *::before,html.${PAUSED} *::after{animation-play-state:paused!important}` +
  "@media (prefers-reduced-motion: reduce){*,*::before,*::after{animation-play-state:paused!important;transition:none!important}}";

/** The whole document for `body`; `paused` for one that never moves, a card's picture. */
export function sceneDocument(body: string, paused = false): string {
  return (
    `<!doctype html><html${paused ? ` class="${PAUSED}"` : ""}><head>` +
    `<meta http-equiv="Content-Security-Policy" content="${SCENE_CSP}">` +
    `<meta charset="utf-8"><style>${SHELL_STYLE}</style></head><body>${body}</body></html>`
  );
}

/**
 * Whether the scene should stand still: always while the window is hidden,
 * and while it is not the one in use unless the user asked for motion then too.
 */
export const shouldPause = (hidden: boolean, focused: boolean, animateInBackground: boolean) =>
  hidden || (!focused && !animateInBackground);
