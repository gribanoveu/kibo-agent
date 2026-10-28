// A body made readable, as an HTTP client shows it (docs/45-http-client.md).
// JSON is re-indented without being parsed: a number keeps its digits
// (`1250.00`, an id past 2^53), and a Postman `{{variable}}` or comment stays
// where it was written. XML goes through xml-formatter.
import xmlFormat from "xml-formatter";

/**
 * Where the piece starting at `i` ends, if one does: a string, a `{{variable}}`,
 * a `//` comment (before its line break) or a `/* *\/` one. Its insides are
 * not JSON's structure and are copied as they are.
 */
function pieceEnd(text: string, i: number): number | null {
  const two = text.slice(i, i + 2);
  if (text[i] === '"') {
    let j = i + 1;
    while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
    return Math.min(j + 1, text.length);
  }
  const close = two === "{{" ? "}}" : two === "/*" ? "*/" : two === "//" ? "\n" : null;
  if (close === null) return null;
  const end = text.indexOf(close, i + 2);
  return end < 0 ? text.length : close === "\n" ? end : end + close.length;
}

const isComment = (piece: string) => piece.startsWith("//") || piece.startsWith("/*");

/**
 * The text without its `//` and `/* *\/` comments — what Postman sends of a JSON
 * body. A comment that is a line of its own goes with its line.
 */
export function stripJsonComments(text: string): string {
  let out = "";
  for (let i = 0; i < text.length; ) {
    const end = pieceEnd(text, i);
    if (end === null) out += text[i++];
    else {
      const piece = text.slice(i, end);
      i = end;
      if (!isComment(piece)) out += piece;
      else {
        const rest = /^[ \t]*(\n|$)/.exec(text.slice(i));
        if (rest && /(^|\n)[ \t]*$/.test(out)) {
          out = out.replace(/[ \t]*$/, "");
          i += rest[0].length;
        }
      }
    }
  }
  return out;
}

/**
 * JSON one value per line, nested by `indent`. Not parsed, so what it does not
 * understand it keeps; text that is not an object or array comes back as it was.
 */
export function formatJson(text: string, indent = "  "): string {
  if (!/^\s*[[{]/.test(stripJsonComments(text))) return text;
  let out = "";
  let depth = 0;
  // A line break in the source since the last piece: a comment after it is on a line of its own.
  let broken = false;
  // A new line at the current depth; twice in a row is still one.
  const newline = () => {
    out = out.replace(/[ \t]*$/, "").replace(/\n$/, "") + "\n" + indent.repeat(depth);
  };
  for (let i = 0; i < text.length; ) {
    const c = text[i];
    const end = pieceEnd(text, i);
    if (end !== null) {
      const piece = text.slice(i, end);
      // Where a line was to end — after a comma or an opening bracket.
      const atLineEnd = /[,[{]\s*$/.test(out);
      // A comment after a value stays on that value's line.
      if (isComment(piece) && !broken && out.trim()) out = out.replace(/\s*$/, " ");
      out += piece;
      i = end;
      if (isComment(piece) && (atLineEnd || /^[ \t]*(\n|$)/.test(text.slice(i)))) newline();
      else if (piece.startsWith("/*")) out += " ";
      broken = false;
      continue;
    }
    i++;
    if (/\s/.test(c)) {
      if (c === "\n") broken = true;
      continue;
    }
    broken = false;
    if (c === "{" || c === "[") {
      const after = text.slice(i).search(/\S/);
      if (text[i + after] === (c === "{" ? "}" : "]")) {
        out += c + text[i + after];
        i += after + 1;
      } else {
        out += c;
        depth++;
        newline();
      }
    } else if (c === "}" || c === "]") {
      depth = Math.max(0, depth - 1);
      newline();
      out += c;
    } else if (c === ",") {
      out += c;
      newline();
    } else if (c === ":") out += ": ";
    else out += c;
  }
  return out.trimEnd();
}

/**
 * XML one element per line, nested by `indent`. Strict: text that does not
 * parse — a tag left open — comes back as it was, not mended into other XML.
 */
export function formatXml(text: string, indent = "  "): string {
  return xmlFormat(text, { indentation: indent, collapseContent: true, lineSeparator: "\n", strictMode: true, throwOnFailure: false });
}
