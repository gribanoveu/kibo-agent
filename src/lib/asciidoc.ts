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
  const doc = await load(text, { safe: "server", extension_registry: registry, attributes: { showtitle: true } });
  return doc.convert();
}
