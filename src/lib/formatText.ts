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

/** Past the end of `i`'s line, if only spaces lie between; null if something else does. */
function endsLine(text: string, i: number): number | null {
  const rest = /[ \t]*(\n|$)/y;
  rest.lastIndex = i;
  return rest.exec(text) ? rest.lastIndex : null;
}

/**
 * Text written piece by piece, whose end is looked at and trimmed without
 * rereading the whole — a regex over all of it per piece made a 100 KB body
 * take seconds.
 */
class Written {
  private parts: string[] = [];
  push(s: string) {
    if (s) this.parts.push(s);
  }
  /** Drops up to `max` characters at the end that `drop` matches, one character each. */
  trim(drop: RegExp, max = Infinity) {
    while (max > 0 && this.parts.length > 0) {
      const last = this.parts[this.parts.length - 1];
      let k = last.length;
      while (max > 0 && k > 0 && drop.test(last[k - 1])) {
        k--;
        max--;
      }
      if (k > 0) {
        this.parts[this.parts.length - 1] = last.slice(0, k);
        return;
      }
      this.parts.pop();
    }
  }
  /** The last character `skip` does not match; "" if there is none. */
  lastNot(skip: RegExp): string {
    for (let p = this.parts.length - 1; p >= 0; p--) {
      const part = this.parts[p];
      for (let k = part.length - 1; k >= 0; k--) if (!skip.test(part[k])) return part[k];
    }
    return "";
  }
  toString() {
    return this.parts.join("");
  }
}

/**
 * The text without its `//` and `/* *\/` comments — what Postman sends of a JSON
 * body. A comment that is a line of its own goes with its line.
 */
export function stripJsonComments(text: string): string {
  const out = new Written();
  let copied = 0;
  for (let i = 0; i < text.length; ) {
    const end = pieceEnd(text, i);
    if (end === null || !isComment(text.slice(i, end))) {
      i = end ?? i + 1;
      continue;
    }
    out.push(text.slice(copied, i));
    copied = i = end;
    const lineEnd = endsLine(text, i);
    if (lineEnd !== null && ["\n", ""].includes(out.lastNot(/[ \t]/))) {
      out.trim(/[ \t]/);
      copied = i = lineEnd;
    }
  }
  out.push(text.slice(copied));
  return out.toString();
}

/**
 * JSON one value per line, nested by `indent`. Not parsed, so what it does not
 * understand it keeps; text that is not an object or array comes back as it was.
 */
export function formatJson(text: string, indent = "  "): string {
  if (!/^\s*[[{]/.test(stripJsonComments(text))) return text;
  const out = new Written();
  let depth = 0;
  // A line break in the source since the last piece: a comment after it is on a line of its own.
  let broken = false;
  // A new line at the current depth; twice in a row is still one.
  const newline = () => {
    out.trim(/[ \t]/);
    out.trim(/\n/, 1);
    out.push("\n" + indent.repeat(depth));
  };
  const space = /\s*/y;
  for (let i = 0; i < text.length; ) {
    const c = text[i];
    const end = pieceEnd(text, i);
    if (end !== null) {
      const piece = text.slice(i, end);
      const last = out.lastNot(/\s/);
      if (isComment(piece) && last) {
        // A comment on a line of its own stays there; one after a value stays on that value's line.
        if (broken) newline();
        else {
          out.trim(/\s/);
          out.push(" ");
        }
      }
      out.push(piece);
      i = end;
      // A line was to end after a comma or an opening bracket.
      if (isComment(piece) && ((last !== "" && ",[{".includes(last)) || endsLine(text, i) !== null)) newline();
      else if (piece.startsWith("/*")) out.push(" ");
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
      space.lastIndex = i;
      space.exec(text);
      if (text[space.lastIndex] === (c === "{" ? "}" : "]")) {
        out.push(c + text[space.lastIndex]);
        i = space.lastIndex + 1;
      } else {
        out.push(c);
        depth++;
        newline();
      }
    } else if (c === "}" || c === "]") {
      depth = Math.max(0, depth - 1);
      newline();
      out.push(c);
    } else if (c === ",") {
      out.push(c);
      newline();
    } else if (c === ":") out.push(": ");
    else out.push(c);
  }
  return out.toString().trimEnd();
}

/**
 * XML one element per line, nested by `indent`. Strict: text that does not
 * parse — a tag left open — comes back as it was, not mended into other XML.
 */
export function formatXml(text: string, indent = "  "): string {
  return xmlFormat(text, { indentation: indent, collapseContent: true, lineSeparator: "\n", strictMode: true, throwOnFailure: false });
}
