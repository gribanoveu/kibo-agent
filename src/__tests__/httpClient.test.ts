import { describe, expect, test } from "bun:test";
import { effectiveAuth, preview, queryOf, resolveRequest, scope, toCurl, variablesIn, withQuery, type Collection, type Environment } from "../lib/httpClient";

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

  test("the Params tab and the address stay in step; a disabled row leaves the address", () => {
    const url = "{{baseUrl}}/o?status=paid&flag";
    expect(queryOf(url)).toEqual([
      { key: "status", value: "paid" },
      { key: "flag", value: "" },
    ]);
    expect(withQuery(url, [{ key: "status", value: "new" }, { key: "flag", value: "", disabled: true }])).toBe("{{baseUrl}}/o?status=new");
    expect(withQuery(url, [])).toBe("{{baseUrl}}/o");
  });
});
