import type { AbstractBlock, BlockMacroProcessorDslInterface } from "@asciidoctor/core";

/**
 * The folder's path `target` names from the file `from`, as an `include::`
 * means it: beside `from`, or from the folder's top when it starts with `/`.
 * `null` when it climbs out of the folder.
 */
export function includePath(from: string, target: string): string | null {
  const parts = target.startsWith("/") ? [] : from.split("/").slice(0, -1);
  for (const part of target.replace(/\\/g, "/").split("/")) {
    if (part === "" || part === ".") continue;
    if (part !== "..") parts.push(part);
    else if (parts.pop() === undefined) return null;
  }
  return parts.join("/");
}

/**
 * `text`, the file at `path`, as HTML: its includes put in, each read by
 * `read` — `null` for one that cannot be, which then says so where it stood.
 * Untrusted: what a document writes as HTML passes through.
 */
export async function asciidocHtml(text: string, path: string, read: (path: string) => Promise<string | null>) {
  // A megabyte of converter, loaded the first time a document is shown.
  const { Extensions, load, LoggerManager, NullLogger } = await import("@asciidoctor/core");
  // What it would report goes to the console, and the page shows it anyway.
  LoggerManager.setLogger(new NullLogger());
  const registry = Extensions.create();
  registry.includeProcessor(function () {
    this.process(async (_doc, reader, target, attrs) => {
      // The file the include is in: this one, or one included before it.
      const resolved = includePath(reader.file ?? path, target);
      const content = resolved === null ? null : await read(resolved).catch(() => null);
      // ponytail: `lines=` and `tag=` are not applied — the whole file goes in.
      reader.pushInclude(content ?? `Unresolved include: ${target}`, resolved, resolved, 1, attrs);
    });
  });
  // A PlantUML diagram — `[plantuml]` over a block, or `plantuml::file.puml[]`
  // — is kept as its source, marked for the preview to draw.
  registry.block("plantuml", function () {
    this.onContexts("listing", "literal", "open");
    this.process(function (parent, reader, attrs) {
      return this.createBlock(parent, "listing", reader.getLines(), { ...attrs, role: "plantuml" });
    });
  });
  // The types have a macro's block come back at once; the parser awaits it,
  // which reading the file takes.
  const diagramFile = async function (this: BlockMacroProcessorDslInterface, parent: AbstractBlock, target: string, attrs: object) {
    // ponytail: from the file shown, not from an included one the macro is in.
    const resolved = includePath(path, target);
    const source = resolved === null ? null : await read(resolved).catch(() => null);
    return source === null
      ? this.createBlock(parent, "paragraph", `Unresolved diagram: ${target}`, {})
      : this.createBlock(parent, "listing", source, { ...attrs, role: "plantuml" });
  };
  registry.blockMacro("plantuml", function () {
    this.process(diagramFile as never);
  });
  const doc = await load(text, { safe: "server", extension_registry: registry, attributes: { showtitle: true } });
  return doc.convert();
}
