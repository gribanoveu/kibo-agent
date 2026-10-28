import { describe, expect, test } from "bun:test";
import {
  addRequest,
  asHttpText,
  cutForHistory,
  effectiveAuth,
  findRequest,
  HISTORY_BODY_LIMIT,
  POSTMAN_SCHEMA,
  preview,
  queryOf,
  removeNode,
  renameNode,
  requestsUnder,
  resolveRequest,
  scope,
  toCurl,
  toPostman,
  variablesIn,
  withQuery,
  type Collection,
  type Environment,
  type HttpRequest,
} from "../lib/httpClient";

const env: Environment = {
  id: "t",
  name: "test",
  agent: "off",
  trustedCert: false,
  variables: [
    { key: "baseUrl", value: "https://api.test", secret: false, enabled: true },
    { key: "token", value: "s3cret", secret: true, enabled: true },
    { key: "old", value: "x", secret: false, enabled: false },
  ],
};

const collection: Collection = {
  id: "c",
  name: "API",
  auth: { type: "bearer", token: "{{token}}" },
  variables: [
    { key: "baseUrl", value: "http://collection" },
    { key: "id", value: "42" },
  ],
  scripts: {},
  items: [
    {
      kind: "folder",
      id: "f",
      name: "Keys",
      auth: { type: "apikey", key: "X-Api-Key", value: "{{key}}", in: "header" },
      items: [{ kind: "request", id: "r1", name: "a", method: "GET", url: "", headers: [], auth: { type: "inherit" }, body: { mode: "none" }, scripts: {} }],
    },
    { kind: "request", id: "r2", name: "b", method: "GET", url: "", headers: [], auth: { type: "inherit" }, body: { mode: "none" }, scripts: {} },
    { kind: "request", id: "r3", name: "c", method: "GET", url: "", headers: [], auth: { type: "noauth" }, body: { mode: "none" }, scripts: {} },
  ],
};

describe("http client rules", () => {
  test("the environment wins over the collection, a secret is masked, a missing one is named", () => {
    const vars = scope(collection, env);
    expect(preview("{{baseUrl}}/o/{{id}}?t={{token}}&x={{old}}&g={{$guid}}", vars)).toEqual({
      text: "https://api.test/o/42?t=•••&x={{old}}&g={{$guid}}",
      missing: ["old"],
    });
  });

  test("variables are listed once each, dynamic ones left out", () => {
    expect(variablesIn("{{a}}/{{ b }}/{{a}}/{{$timestamp}}")).toEqual(["a", "b"]);
  });

  test("auth comes from the nearest folder, then the collection; a request's own wins", () => {
    expect(effectiveAuth(collection, "r1")?.from).toBe("Keys");
    expect(effectiveAuth(collection, "r2")).toEqual({ auth: collection.auth, from: "API" });
    expect(effectiveAuth(collection, "r3")?.auth).toEqual({ type: "noauth" });
  });

  test("a request leaves with its folder's auth, the implied Content-Type and a disabled header left out", () => {
    const request = {
      id: "r1",
      name: "a",
      method: "POST" as const,
      url: "{{baseUrl}}/o/{{id}}",
      headers: [{ key: "X-Debug", value: "1", disabled: true }, { key: "Accept", value: "application/json" }],
      auth: { type: "inherit" as const },
      body: { mode: "raw" as const, language: "json" as const, raw: '{"t":"{{token}}"}' },
      scripts: {},
    };
    const env2 = { ...env, variables: [...env.variables, { key: "key", value: "k-1", secret: false, enabled: true }] };
    expect(resolveRequest(collection, request, env2)).toEqual({
      method: "POST",
      url: "https://api.test/o/42",
      headers: [
        { key: "Accept", value: "application/json" },
        { key: "X-Api-Key", value: "k-1" },
        { key: "Content-Type", value: "application/json" },
      ],
      body: '{"t":"•••"}',
      missing: [],
      notes: [],
    });
  });

  test("the cURL names a secret instead of carrying it, and quotes what the shell would split", () => {
    const request = {
      id: "r2",
      name: "b",
      method: "GET" as const,
      url: "{{baseUrl}}/o?q=it's",
      headers: [],
      auth: { type: "inherit" as const },
      body: { mode: "none" as const },
      scripts: {},
    };
    const curl = toCurl(resolveRequest(collection, request, env, "name"));
    expect(curl).toBe("curl 'https://api.test/o?q=it'\\''s' \\\n  -H 'Authorization: Bearer {{token}}'");
    expect(curl).not.toContain("s3cret");
    const post = toCurl(resolveRequest(collection, { ...request, method: "DELETE", body: { mode: "raw", language: "text", raw: "x" } }, env, "name"));
    expect(post.startsWith("curl -X DELETE ")).toBe(true);
    expect(post).toContain("--data-raw 'x'");
  });

  test("a variable nowhere and a body kept from Postman are said, not sent silently", () => {
    const request = {
      id: "r3",
      name: "c",
      method: "POST" as const,
      url: "{{baseUrl}}/{{nowhere}}",
      headers: [],
      auth: { type: "noauth" as const },
      body: { mode: "other" as const, name: "formdata" },
      scripts: {},
    };
    const r = resolveRequest(collection, request, env);
    expect(r.missing).toEqual(["nowhere"]);
    expect(r.notes).toEqual(["a formdata body is kept from Postman and not sent yet"]);
  });

  test("history cuts a long body and says so, and keeps a short one as it is", () => {
    expect(cutForHistory("{}")).toBe("{}");
    const long = "x".repeat(HISTORY_BODY_LIMIT + 10);
    const cut = cutForHistory(long);
    expect(cut.startsWith("x".repeat(HISTORY_BODY_LIMIT))).toBe(true);
    expect(cut).not.toContain("x".repeat(HISTORY_BODY_LIMIT + 1));
    expect(cut).toContain("cut at 256.0 KB");
  });

  test("a message reads as HTTP writes it: a blank line before the body, none without one", () => {
    const headers = [{ key: "Accept", value: "*/*" }];
    expect(asHttpText("GET /a", headers, null)).toBe("GET /a\nAccept: */*");
    expect(asHttpText("HTTP/1.1 200 OK", headers, "{}")).toBe("HTTP/1.1 200 OK\nAccept: */*\n\n{}");
  });

  test("the Params tab and the address stay in step; a disabled row leaves the address", () => {
    const url = "{{baseUrl}}/o?status=paid&flag";
    expect(queryOf(url)).toEqual([
      { key: "status", value: "paid" },
      { key: "flag", value: "" },
    ]);
    expect(withQuery(url, [{ key: "status", value: "new" }, { key: "flag", value: "", disabled: true }])).toBe("{{baseUrl}}/o?status=new");
    expect(withQuery(url, [])).toBe("{{baseUrl}}/o");
  });

  test("a node is renamed, added to or removed at any depth, with what it holds", () => {
    const fresh: HttpRequest = { id: "n", name: "New", method: "GET", url: "", headers: [], auth: { type: "inherit" }, body: { mode: "none" }, scripts: {} };
    const tree = [collection, { ...collection, id: "c2", name: "Other", items: [] }];
    expect(renameNode(tree, "c", "Renamed")[0].name).toBe("Renamed");
    expect(findRequest(renameNode(tree, "r1", "deep"), "r1")?.request.name).toBe("deep");
    expect(findRequest(renameNode(tree, "f", "Folder"), "r1")?.path).toEqual(["API", "Folder"]);
    expect(findRequest(addRequest(tree, "f", fresh), "n")?.path).toEqual(["API", "Keys"]);
    expect(findRequest(addRequest(tree, "c2", fresh), "n")?.collection.id).toBe("c2");
    expect(addRequest(tree, "r2", fresh)).toEqual(tree);
    expect(requestsUnder(tree, "c")).toEqual(["r1", "r2", "r3"]);
    expect(requestsUnder(tree, "f")).toEqual(["r1"]);
    expect(requestsUnder(tree, "r3")).toEqual(["r3"]);
    expect(requestsUnder(tree, "c2")).toEqual([]);
    expect(findRequest(removeNode(tree, "f"), "r1")).toBeNull();
    expect(findRequest(removeNode(tree, "f"), "r2")).not.toBeNull();
    expect(removeNode(tree, "c").map((c) => c.id)).toEqual(["c2"]);
  });

  test("a collection goes out as a Postman v2.1 file: auth, body, scripts and variables in Postman's shapes", () => {
    const withMore: Collection = {
      ...collection,
      scripts: { prerequest: "a();\nb();", test: " " },
      items: [
        ...collection.items,
        {
          kind: "request",
          id: "r4",
          name: "post",
          method: "POST",
          url: "{{baseUrl}}/o",
          headers: [{ key: "X-Off", value: "1", disabled: true }],
          auth: { type: "basic", username: "u", password: "p" },
          body: { mode: "raw", language: "json", raw: "{}" },
          scripts: { test: "pm.test();" },
        },
      ],
    };
    const text = toPostman(withMore);
    expect(text).toContain('\n\t"info"');
    const file = JSON.parse(text);
    expect(file.info).toEqual({ _postman_id: "c", name: "API", schema: POSTMAN_SCHEMA });
    expect(file.auth).toEqual({ type: "bearer", bearer: [{ key: "token", value: "{{token}}", type: "string" }] });
    expect(file.event).toEqual([{ listen: "prerequest", script: { type: "text/javascript", exec: ["a();", "b();"] } }]);
    expect(file.variable).toEqual([
      { key: "baseUrl", value: "http://collection" },
      { key: "id", value: "42" },
    ]);
    const [folder, inherit, , post] = file.item;
    expect(folder.item[0].name).toBe("a");
    expect(folder.auth.apikey).toContainEqual({ key: "in", value: "header", type: "string" });
    expect(inherit.request.auth).toBeUndefined();
    expect(inherit.request.body).toBeUndefined();
    expect(post.request).toEqual({
      auth: { type: "basic", basic: [{ key: "username", value: "u", type: "string" }, { key: "password", value: "p", type: "string" }] },
      method: "POST",
      header: [{ key: "X-Off", value: "1", disabled: true }],
      body: { mode: "raw", raw: "{}", options: { raw: { language: "json" } } },
      url: "{{baseUrl}}/o",
    });
    expect(post.event[0].listen).toBe("test");
  });
});
