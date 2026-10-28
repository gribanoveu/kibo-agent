import { describe, expect, test } from "bun:test";
import { formatJson, formatXml, stripJsonComments } from "../lib/formatText";

describe("formatting a body", () => {
  test("JSON is indented one value per line, keeping digits, variables and strings as written", () => {
    const text = '{"amount":1250.00,"id":12345678901234567890,"to":{{customerId}},"note":"a, b: {c} // d \\" e","items":[{"sku":"A"},2],"tags":[ ],"meta":{}}';
    expect(formatJson(text)).toBe(
      [
        "{",
        '  "amount": 1250.00,',
        '  "id": 12345678901234567890,',
        '  "to": {{customerId}},',
        '  "note": "a, b: {c} // d \\" e",',
        '  "items": [',
        "    {",
        '      "sku": "A"',
        "    },",
        "    2",
        "  ],",
        '  "tags": [],',
        '  "meta": {}',
        "}",
      ].join("\n"),
    );
    expect(formatJson(text, "\t")).toContain('\n\t"items": [\n\t\t{\n\t\t\t"sku"');
  });

  test("a comment stays on its value's line or its own, and a commented-out block keeps its lines", () => {
    const text = '{"a": 1, // after a\n  // own line\n"b": [ /* first */ 2, /* last */ ],\n/* "c": 3,\n   "d": 4 */\n"e": /* ms */ 5}';
    expect(formatJson(text)).toBe(
      [
        "{",
        '  "a": 1, // after a',
        "  // own line",
        '  "b": [ /* first */',
        "    2, /* last */",
        "  ],",
        '  /* "c": 3,',
        '   "d": 4 */',
        '  "e": /* ms */ 5',
        "}",
      ].join("\n"),
    );
  });

  test("formatted twice is formatted once; what is not an object or array comes back as it was", () => {
    const once = formatJson('{"a":[1,{"b":2}], // c\n"d":3}');
    expect(formatJson(once)).toBe(once);
    expect(formatJson("plain {text}, with: commas")).toBe("plain {text}, with: commas");
    expect(formatJson("// only a note\n[1]")).toBe("// only a note\n[\n  1\n]");
  });

  test("comments go, strings and variables that look like them stay", () => {
    const text = '{"url": "http://x//y", /* gone */ "v": {{a}} // gone\n}';
    expect(stripJsonComments(text)).toBe('{"url": "http://x//y",  "v": {{a}} \n}');
    expect(stripJsonComments('{\n  // "a": 1,\n  /* "b": 2,\n  "c": 3 */\n  "d": 4 // after\n}')).toBe('{\n  "d": 4 \n}');
    expect(stripJsonComments('"unclosed /* x')).toBe('"unclosed /* x');
    expect(stripJsonComments("[1] /* unclosed")).toBe("[1] ");
  });

  test("XML is indented; XML that does not parse is not mended", () => {
    expect(formatXml('<?xml version="1.0"?><a x="1"><b>t</b><c/></a>')).toBe('<?xml version="1.0"?>\n<a x="1">\n  <b>t</b>\n  <c/>\n</a>');
    expect(formatXml("<a><b></a>")).toBe("<a><b></a>");
  });
});
