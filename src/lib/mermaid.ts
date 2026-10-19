// A ```mermaid block drawn as the diagram it describes — as an image, like
// PlantUML's, so its SVG runs nothing and styles nothing outside it, and the
// same picture opens larger in `DiagramView`.
//
// Pele draws it: Mermaid's syntax, a fraction of Mermaid's size, no DOM. It is
// still loaded the first time a diagram is shown, as most answers have none.
// It escapes the text it draws and filters links and styles; the <img> it is
// shown in runs nothing whatever got through.

type Pele = typeof import("pele");

let loading: Promise<Pele> | null = null;

function load(): Promise<Pele> {
  loading ??= import("pele").catch((error: unknown) => {
    // A failed load is tried again by the next diagram, not remembered.
    loading = null;
    throw error;
  });
  return loading;
}

/** Pele's palette variable, and the app's token it takes its colour from. */
const PALETTE: Array<[pele: string, token: string]> = [
  ["bg", "--bg-panel-soft"],
  ["fg", "--text"],
  ["muted", "--text-dim"],
  ["line", "--text-dim"],
  ["surface", "--bg-hover"],
  ["surface-alt", "--bg-active"],
  ["border", "--text-faint"],
  ["accent", "--accent"],
  ["error", "--red"],
  ["critical", "--red"],
  ["series-1", "--accent"],
  ["series-2", "--term-blue"],
  ["series-3", "--green"],
  ["series-4", "--amber"],
  ["series-5", "--purple"],
  ["series-6", "--term-cyan"],
  ["series-7", "--term-magenta"],
  ["series-8", "--red"],
  ["font", "--font-ui"],
  ["font-mono", "--font-mono"],
];

/** The SVG for `source`, in the window's theme. Throws what Pele says is wrong with it. */
export async function renderMermaid(source: string): Promise<string> {
  const pele = await load();
  const tokens = getComputedStyle(document.documentElement);
  const value = (token: string) => tokens.getPropertyValue(token).trim();
  const font = value("--font-ui");
  const mono = value("--font-mono");
  const { svg } = pele.render(source, {
    // Fixed at the size it was laid out at, so a zoom has something to start from.
    responsive: false,
    ...(font ? { fontFamily: font } : {}),
    ...(mono ? { fontFamilyMono: mono } : {}),
  });
  return themed(svg, PALETTE.map(([name, token]) => [name, value(token)]));
}

/**
 * Pele colours an SVG through `var(--pele-*)`, set by the page around it. An
 * <img> has no page around it, so the values are written on its root instead;
 * one the theme leaves empty keeps Pele's own.
 */
export function themed(svg: string, palette: Array<[name: string, value: string]>): string {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = doc.documentElement;
  if (root.nodeName !== "svg") return svg;
  const vars = palette.filter(([, v]) => v).map(([name, v]) => `--pele-${name}:${v};`).join("");
  root.setAttribute("style", vars + (root.getAttribute("style") ?? ""));
  return new XMLSerializer().serializeToString(doc);
}
