import { frostFerns } from "./frostFerns";
import { glacierMist } from "./glacierMist";
import { snowfall } from "./snowfall";

/** A scene the app ships: its body is HTML and CSS for `sceneDocument`. */
export type BuiltinScene = { id: string; title: string; body: () => string };

/** In the order the Frost group in Settings offers them. */
export const BUILTIN_SCENES: readonly BuiltinScene[] = [
  { id: "frost-ferns", title: "Frost Ferns", body: frostFerns },
  { id: "glacier-mist", title: "Glacier Mist", body: glacierMist },
  { id: "snowfall", title: "Snowfall", body: snowfall },
];
