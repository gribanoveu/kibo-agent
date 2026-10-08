// A ```mermaid block drawn as the diagram it describes — as an image, like
// PlantUML's, so its SVG runs nothing and styles nothing outside it, and the
// same picture opens larger in `DiagramView`.
//
// Mermaid is loaded the first time a diagram is shown: it is most of a
// megabyte, and most answers have none. `strict` has DOMPurify clean the SVG
// and keeps scripts and click handlers out of it, and an `%%{init}%%` line in
// the model's text cannot loosen that — `securityLevel` is one of Mermaid's
// `secure` keys by default.

type Mermaid = (typeof import("mermaid"))["default"];

let loading: Promise<Mermaid> | null = null;
let rendered = 0;

function load(): Promise<Mermaid> {
  loading ??= import("mermaid").then(
    (module) => module.default,
    (error) => {
      // A failed load is tried again by the next diagram, not remembered.
      loading = null;
      throw error;
    },
  );
  return loading;
}

/** The SVG for `source`, in the light or dark palette. Throws what Mermaid says is wrong with it. */
export async function renderMermaid(source: string, dark: boolean): Promise<string> {
  const mermaid = await load();
  const font = getComputedStyle(document.documentElement).getPropertyValue("--font-ui").trim();
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: dark ? "dark" : "default",
    // Labels as SVG text: HTML in a `foreignObject` is not something every
    // webview draws inside an <img>.
    htmlLabels: false,
    ...(font ? { fontFamily: font } : {}),
  });
  // Parsed first: on a syntax error `render` can draw Mermaid's own error
  // picture instead of throwing, and that is not the diagram anyone asked for.
  await mermaid.parse(source);
  const id = `kibo-mermaid-${++rendered}`;
  try {
    return sized((await mermaid.render(id, source)).svg);
  } finally {
    // Mermaid measures text in a scratch element it can leave behind on failure.
    document.getElementById(`d${id}`)?.remove();
  }
}

/**
 * Mermaid's SVG says `width="100%"` and caps it with a style: an <img> of it
 * has no size of its own, so a zoom has nothing to start from. Its viewBox
 * is the size it was laid out at.
 */
export function sized(svg: string): string {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = doc.documentElement;
  const box = root.getAttribute("viewBox")?.trim().split(/[\s,]+/).map(Number);
  if (root.nodeName !== "svg" || !box || box.length !== 4 || box.some((n) => !Number.isFinite(n))) return svg;
  root.setAttribute("width", String(Math.ceil(box[2])));
  root.setAttribute("height", String(Math.ceil(box[3])));
  const style = (root.getAttribute("style") ?? "").replace(/max-width:[^;]*;?/i, "").trim();
  if (style) root.setAttribute("style", style);
  else root.removeAttribute("style");
  return new XMLSerializer().serializeToString(doc);
}
