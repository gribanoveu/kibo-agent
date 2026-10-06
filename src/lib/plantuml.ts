import vizUrl from "@plantuml/core/viz-global.js?url";

// Seven megabytes of engine and Graphviz, loaded the first time a diagram is
// drawn. Graphviz is a classic script the engine finds on `window`.
let engine: Promise<typeof import("@plantuml/core")> | null = null;
const load = () =>
  (engine ??= new Promise<void>((resolve, reject) => {
    const script = document.createElement("script");
    script.src = vizUrl;
    script.onload = () => resolve();
    script.onerror = () => reject(new Error("the diagram engine did not load"));
    document.head.append(script);
  })
    .then(() => import("@plantuml/core"))
    .catch((e: unknown) => {
      engine = null;
      throw e;
    }));

// One at a time: the engine keeps its state between renders.
let queue: Promise<unknown> = Promise.resolve();

/** A PlantUML diagram's source as SVG; `@startuml`/`@enduml` are added when it has none. */
export function plantumlSvg(source: string): Promise<string> {
  const text = /^\s*@start/.test(source) ? source : `@startuml\n${source}\n@enduml`;
  const run = queue
    .then(load)
    .then(({ renderToString }) => new Promise<string>((resolve, reject) => renderToString(text.split(/\r?\n/), resolve, (e) => reject(new Error(e)))));
  queue = run.catch(() => {});
  return run;
}
