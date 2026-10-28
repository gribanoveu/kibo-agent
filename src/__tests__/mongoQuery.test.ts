import { describe, expect, test } from "bun:test";
import { apply, erased, knownFields, suggest, typed } from "../lib/mongoQuery";
import type { MongoDocument } from "../lib/mongo";

const value = (text: string, bson: string) => ({ kind: "value" as const, text, bson });
const FIELDS = [
  { path: "_id", bson: "ObjectId" },
  { path: "name", bson: "String" },
  { path: "organizationId", bson: "String" },
  { path: "address", bson: "Object" },
  { path: "address.city", bson: "String" },
  { path: "total", bson: "Int32" },
];
const labels = (text: string, caret = text.length, kind: "filter" | "projection" | "sort" = "filter") => suggest(text, caret, FIELDS, kind)?.items.map((s) => s.label) ?? [];

describe("knownFields", () => {
  test("dotted paths, an array's items under its own name, each once", () => {
    const docs: MongoDocument[] = [
      { id: null, text: "", fields: [["a", { kind: "object", fields: [["b", value("1", "Int32")]] }], ["tags", { kind: "array", items: [value('"x"', "String"), { kind: "object", fields: [["n", value("2", "Int32")]] }] }]] },
      { id: null, text: "", fields: [["a", value('"s"', "String")]] },
    ];
    expect(knownFields(docs)).toEqual([
      { path: "a", bson: "Object" },
      { path: "a.b", bson: "Int32" },
      { path: "tags", bson: "Array" },
      { path: "tags.n", bson: "Int32" },
    ]);
  });
});

describe("suggest", () => {
  test("a key typed gets the fields starting with it first, then those holding it", () => {
    expect(labels("{ org")).toEqual(["organizationId"]);
    expect(labels("{ a")).toEqual(["address", "address.city", "name", "organizationId", "total"]);
    expect(suggest("{ org", 5, FIELDS, "filter")).toEqual({ from: 2, to: 5, items: [{ label: "organizationId", detail: "String", insert: 'organizationId: "|"' }] });
  });

  test("bare text brings its braces; a dotted path is quoted; an object's value opens one", () => {
    expect(suggest("org", 3, FIELDS, "filter")!.items[0].insert).toBe('{ organizationId: "|" }');
    expect(suggest("{ x: 1, address.c", 17, FIELDS, "filter")!.items[0].insert).toBe('"address.city": "|"');
    expect(suggest("{ addr", 6, FIELDS, "filter")!.items[0].insert).toBe("address: { | }");
  });

  test("a key that has its value already is only renamed", () => {
    expect(suggest('{ na: { $ne: "value" } }', 4, FIELDS, "filter")).toMatchObject({ from: 2, to: 4, items: [{ label: "name", insert: "name" }] });
    expect(suggest("{ total: { $g : 5 } }", 13, FIELDS, "filter")!.items.map((s) => s.insert)).toEqual(["$gt", "$gte"]);
  });

  test("a projection or a sort takes 1 for a field", () => {
    expect(suggest("{ tot", 5, FIELDS, "sort")!.items[0].insert).toBe("total: 1|");
  });

  test("$ in a filter is an operator, elsewhere nothing", () => {
    expect(labels("{ total: { $g")).toEqual(["$gt", "$gte"]);
    expect(labels("{ $o")).toEqual(["$or"]);
    expect(labels("{ $o", 4, "projection")).toEqual([]);
  });

  test("nothing in a value, inside a string, or with no word", () => {
    expect(labels("{ name: na")).toEqual([]);
    expect(labels('{ name: "{ na')).toEqual([]);
    expect(labels("{ ")).toEqual([]);
    expect(labels("{ zzz")).toEqual([]);
  });
});

test("apply replaces the whole word and puts the caret at |", () => {
  expect(apply("{ orgX }", { from: 2, to: 6 }, 'organizationId: "|"')).toEqual({ text: '{ organizationId: "" }', start: 19, end: 19 });
  expect(apply("{ x", { from: 2, to: 3 }, "x: 1")).toEqual({ text: "{ x: 1", start: 6, end: 6 });
});

describe("typed", () => {
  test("an opener brings its closer, a closer next is stepped over", () => {
    expect(typed("", 0, 0, "{")).toEqual({ text: "{}", start: 1, end: 1 });
    expect(typed("{ a: }", 5, 5, '"')).toEqual({ text: '{ a: ""}', start: 6, end: 6 });
    expect(typed("{}", 1, 1, "}")).toEqual({ text: "{}", start: 2, end: 2 });
  });

  test("a selection is wrapped", () => {
    expect(typed("{ a: x }", 5, 6, "[")).toEqual({ text: "{ a: [x] }", start: 6, end: 7 });
  });

  test("typed as it is: not a pair, inside a string, before a word, a quote after a word", () => {
    expect(typed("{", 1, 1, "a")).toBeNull();
    expect(typed('{ a: "x', 7, 7, "{")).toBeNull();
    expect(typed("{ a }", 2, 2, "[")).toBeNull();
    expect(typed('{ a: "don', 9, 9, "'")).toBeNull();
  });
});

test("erased takes an empty pair, nothing else", () => {
  expect(erased("{}", 1, 1)).toEqual({ text: "", start: 0, end: 0 });
  expect(erased("{a}", 2, 2)).toBeNull();
  expect(erased("{}", 0, 2)).toBeNull();
});
