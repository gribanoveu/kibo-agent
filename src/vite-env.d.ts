/// <reference types="vite/client" />

/** AsciiDoc's mode for CodeMirror 6, as a stream parser; the package ships no types. */
declare module "codemirror-asciidoc" {
  import type { StreamParser } from "@codemirror/language";
  export const asciidoc: StreamParser<unknown>;
}
