import { describe, expect, test } from "bun:test";
import { asciidocHtml, includePath } from "../lib/asciidoc";

describe("includePath", () => {
  test("beside the including file, from the top with a slash, never out of the folder", () => {
    expect(includePath("doc/a.adoc", "b.adoc")).toBe("doc/b.adoc");
    expect(includePath("doc/a.adoc", "./parts/../c.adoc")).toBe("doc/c.adoc");
    expect(includePath("doc/a.adoc", "../top.adoc")).toBe("top.adoc");
    expect(includePath("doc/a.adoc", "/x/y.adoc")).toBe("x/y.adoc");
    expect(includePath("a.adoc", "..\\up.adoc")).toBeNull();
    expect(includePath("doc/a.adoc", "../../up.adoc")).toBeNull();
  });
});

describe("asciidocHtml", () => {
  test("renders, with includes put in — nested ones from the file they are in", async () => {
    const files: Record<string, string> = {
      "doc/parts/one.adoc": "== One\n\ninclude::two.adoc[]\n",
      "doc/parts/two.adoc": "Two's text.\n",
    };
    const read = (path: string) => Promise.resolve(files[path] ?? null);
    const html = await asciidocHtml("= Title\n\ninclude::parts/one.adoc[]\n\ninclude::missing.adoc[]\n", "doc/main.adoc", read);
    expect(html).toContain("<h1>Title</h1>");
    expect(html).toContain("One</h2>");
    expect(html).toContain("Two&#8217;s text.");
    expect(html).toContain("Unresolved include: missing.adoc");
  });
});
