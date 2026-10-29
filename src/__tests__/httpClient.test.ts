import { describe, expect, test } from "bun:test";
import {
  addRequest,
  asHttpText,
  collectionFrom,
  effectiveAuth,
  findRequest,
  ownerOf,
  pretty,
  preview,
  queryOf,
  removeNode,
  renameNode,
  requestsUnder,
  resolveRequest,
  scope,
  shownValues,
  requestFrom,
  requestToPostman,
  toCurl,
  variableParts,
  variableValue,
  variablesIn,
  withParams,
  paramsOf,
  withQuery,
  type Collection,
  type Environment,
  type HttpRequest,
} from "../lib/httpClient";

const env: Environment = {
  id: "t",
  name: "test",
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

  test("a JSON body leaves without its comments, and a variable in one is not missing; a text body keeps them", () => {
    const raw = '{\n  // "debug": {{debug}},\n  "url": "http://x/{{id}}" /* why */\n}';
    const request = {
      id: "r2",
      name: "b",
      method: "POST" as const,
      url: "{{baseUrl}}/o",
      headers: [],
      auth: { type: "noauth" as const },
      body: { mode: "raw" as const, language: "json" as const, raw },
      scripts: {},
    };
    const json = resolveRequest(collection, request, env);
    expect(json.body).toBe('{\n  "url": "http://x/42" \n}');
    expect(json.missing).toEqual([]);
    const text = resolveRequest(collection, { ...request, body: { ...request.body, language: "text" as const } }, env);
    expect(text.body).toContain("// ");
    expect(text.missing).toEqual(["debug"]);
  });

  test("a text is cut at its variables, each found in scope, made when sent, or set nowhere", () => {
    const values = shownValues(scope(collection, env));
    expect(values.get("token")).toBe("•••");
    expect(values.get("baseUrl")).toBe("https://api.test");
    expect(variableParts("{{baseUrl}}/o/{{ id }}?t={{$guid}}&x={{nope}}!", values)).toEqual([
      { text: "{{baseUrl}}", variable: { name: "baseUrl", value: "https://api.test" } },
      { text: "/o/" },
      { text: "{{ id }}", variable: { name: "id", value: "42" } },
      { text: "?t=" },
      { text: "{{$guid}}", variable: { name: "$guid", value: "made as the request is sent" } },
      { text: "&x=" },
      { text: "{{nope}}", variable: { name: "nope", value: null } },
      { text: "!" },
    ]);
    expect(variableParts("no variables {here}", values)).toEqual([{ text: "no variables {here}" }]);
    expect(variableParts("", values)).toEqual([]);
    expect(variableValue("old", values)).toBeNull();
  });

  test("a response keeps every digit; XML is indented, text left as it came", () => {
    expect(pretty('{"amount":1250.00,"id":12345678901234567890}', "application/json")).toBe('{\n  "amount": 1250.00,\n  "id": 12345678901234567890\n}');
    expect(pretty("<a><b>1</b></a>", "application/xml; charset=utf-8")).toBe("<a>\n  <b>1</b>\n</a>");
    expect(pretty("{ a }", "text/plain")).toBe("{ a }");
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

  test("a parameter turned off stays in Params, out of the address, and comes back when turned on", () => {
    const request: HttpRequest = { id: "p", name: "p", method: "GET", url: "/o?a=1&b=2", headers: [], auth: { type: "noauth" }, body: { mode: "none" }, scripts: {} };
    const off = withParams(request, [{ key: "a", value: "1" }, { key: "b", value: "2", disabled: true }]);
    expect(off.url).toBe("/o?a=1");
    expect(paramsOf(off)).toEqual([{ key: "a", value: "1" }, { key: "b", value: "2", disabled: true }]);
    const on = withParams(off, [{ key: "a", value: "1" }, { key: "b", value: "2" }]);
    expect(on.url).toBe("/o?a=1&b=2");
    expect(on.paramsOff).toBeUndefined();
    expect(withQuery("/o", [{ key: "", value: "x" }])).toBe("/o?=x");
  });

  test("the cURL keeps the method curl would change, and Basic with a named secret goes as -u", () => {
    const base = { id: "c", name: "c", url: "/o", headers: [], auth: { type: "noauth" as const }, scripts: {} };
    const get = toCurl(resolveRequest(collection, { ...base, method: "GET", body: { mode: "raw", language: "json", raw: "{}" } }, env, "name"));
    expect(get.startsWith("curl -X GET ")).toBe(true);
    const post = toCurl(resolveRequest(collection, { ...base, method: "POST", body: { mode: "raw", language: "json", raw: "{}" } }, env, "name"));
    expect(post.startsWith("curl '")).toBe(true);
    expect(toCurl(resolveRequest(collection, { ...base, method: "HEAD", body: { mode: "none" } }, env, "name")).startsWith("curl -I ")).toBe(true);
    const basic = toCurl(resolveRequest(collection, { ...base, method: "GET", auth: { type: "basic", username: "me", password: "{{token}}" }, body: { mode: "none" } }, env, "name"));
    expect(basic).toContain("-u 'me:{{token}}'");
    expect(basic).not.toContain("Authorization");
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

  test("globals are under the collection, which is under the environment", () => {
    const globals = [
      { key: "id", value: "from-globals", secret: false, enabled: true },
      { key: "only", value: "g", secret: true, enabled: true },
      { key: "off", value: "x", secret: false, enabled: false },
    ];
    const vars = scope(collection, env, globals);
    expect(vars.get("id")?.value).toBe("42");
    expect(vars.get("baseUrl")?.value).toBe("https://api.test");
    expect(shownValues(vars).get("only")).toBe("•••");
    expect(vars.has("off")).toBe(false);
  });

  test("the collection that holds a node is found at any depth, and a collection holds itself", () => {
    const tree = [collection, { ...collection, id: "c2", items: [] }];
    expect(ownerOf(tree, "r1")?.id).toBe("c");
    expect(ownerOf(tree, "f")?.id).toBe("c");
    expect(ownerOf(tree, "c2")?.id).toBe("c2");
    expect(ownerOf(tree, "nowhere")).toBeNull();
  });
});

describe("a Postman file read and written back", () => {
  const SOURCE = {
    method: "POST",
    header: [
      { key: "Accept", value: "application/json", type: "text", description: "what comes back" },
      { key: "X-Off", value: "1", disabled: true },
    ],
    body: { mode: "raw", raw: '{"a": 1}', options: { raw: { language: "json", headerFamily: "json" } } },
    url: {
      raw: "{{baseUrl}}/orders/:id?page=2",
      host: ["{{baseUrl}}"],
      path: ["orders", ":id"],
      query: [
        { key: "page", value: "2" },
        { key: "debug", value: "1", disabled: true },
      ],
      variable: [{ key: "id", value: "7" }],
    },
    description: "made in Postman",
  };
  const FILE = {
    info: { _postman_id: "x", name: "Shop", schema: "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
    auth: { type: "bearer", bearer: [{ key: "token", value: "{{token}}", type: "string" }] },
    event: [{ listen: "prerequest", script: { type: "text/javascript", exec: ["a();", "b();"] } }],
    variable: [{ key: "page", value: "1" }],
    item: [
      {
        id: "f",
        name: "Orders",
        auth: { type: "oauth2", oauth2: [] },
        item: [{ id: "r", name: "Create", event: [{ listen: "test", script: { exec: "pm.test();" } }], request: SOURCE, response: [{ code: 201 }] }],
      },
      { id: "s", name: "Ping", request: "{{baseUrl}}/ping" },
      { id: "g", name: "GraphQL", request: { method: "post", url: "/g", body: { mode: "graphql", graphql: {} }, auth: { type: "digest" } } },
    ],
  };
  const read = () => collectionFrom({ id: "c", file: structuredClone(FILE), error: null });
  const request = (id: string) => findRequest([read()], id)!.request;

  test("what the page edits is read out of the file, the rest named and kept", () => {
    const c = read();
    expect(c.name).toBe("Shop");
    expect(c.auth).toEqual({ type: "bearer", token: "{{token}}" });
    expect(c.scripts).toEqual({ prerequest: "a();\nb();" });
    expect(c.variables).toEqual([{ key: "page", value: "1" }]);
    expect(c.items[0]).toMatchObject({ kind: "folder", name: "Orders", auth: { type: "other", name: "oauth2" } });
    const r = request("r");
    expect(r).toMatchObject({
      method: "POST",
      url: "{{baseUrl}}/orders/:id?page=2",
      paramsOff: [{ key: "debug", value: "1" }],
      headers: [
        { key: "Accept", value: "application/json" },
        { key: "X-Off", value: "1", disabled: true },
      ],
      auth: { type: "inherit" },
      body: { mode: "raw", language: "json", raw: '{"a": 1}' },
      scripts: { test: "pm.test();" },
    });
    expect(request("s")).toMatchObject({ method: "GET", url: "{{baseUrl}}/ping", body: { mode: "none" } });
    expect(request("g")).toMatchObject({ method: "POST", auth: { type: "other", name: "digest" }, body: { mode: "other", name: "graphql" } });
    const apikey = (where: string) => ({ type: "apikey", apikey: [{ key: "key", value: "k" }, { key: "value", value: "v" }, { key: "in", value: where }] });
    expect(requestFrom("a", "A", { auth: apikey("query") }).auth).toEqual({ type: "apikey", key: "k", value: "v", in: "query" });
    expect(requestFrom("a", "A", { auth: apikey("header") }).auth).toMatchObject({ in: "header" });
    expect(collectionFrom({ id: "bad", file: null, error: "not JSON" })).toMatchObject({ name: "bad", items: [], error: "not JSON" });
  });

  test("a request not edited goes back as it came, to the byte", () => {
    for (const id of ["r", "s", "g"]) expect(requestToPostman(request(id))).toEqual(findSource(id));
    // A method written in lower case reads as upper, and is still not an edit.
    expect(JSON.stringify(requestToPostman(request("g")))).toBe(JSON.stringify(FILE.item[2].request));
  });

  test("an edit is written over what it changes, and the rest keeps its place and its extras", () => {
    const r = request("r");
    const out = requestToPostman({ ...r, url: "{{baseUrl}}/orders/:id?page=3&sort=asc" }) as Record<string, unknown>;
    expect(Object.keys(out)).toEqual(Object.keys(SOURCE));
    expect(out.header).toEqual(SOURCE.header);
    expect(out.body).toEqual(SOURCE.body);
    expect(out.url).toEqual({
      raw: "{{baseUrl}}/orders/:id?page=3&sort=asc",
      host: ["{{baseUrl}}"],
      path: ["orders", ":id"],
      query: [
        { key: "page", value: "3" },
        { key: "sort", value: "asc" },
        { key: "debug", value: "1", disabled: true },
      ],
      variable: [{ key: "id", value: "7" }],
    });

    const headers = requestToPostman({ ...r, headers: [{ key: "Accept", value: "text/xml" }, { key: "X-New", value: "2" }] }) as { header: unknown };
    expect(headers.header).toEqual([
      { key: "Accept", value: "text/xml", type: "text", description: "what comes back" },
      { key: "X-New", value: "2" },
    ]);
    const on = requestToPostman({ ...r, headers: r.headers.map((h) => ({ key: h.key, value: h.value })) }) as { header: unknown[] };
    expect(on.header[1]).toEqual({ key: "X-Off", value: "1" });
    const body = requestToPostman({ ...r, body: { mode: "raw", language: "xml", raw: "<a/>" } }) as { body: unknown };
    expect(body.body).toEqual({ mode: "raw", raw: "<a/>", options: { raw: { language: "xml", headerFamily: "json" } } });
    const none = requestToPostman({ ...r, body: { mode: "none" }, auth: { type: "noauth" } }) as Record<string, unknown>;
    expect("body" in none).toBe(false);
    expect(none.auth).toEqual({ type: "noauth" });
    const inherit = requestToPostman({ ...request("g"), auth: { type: "inherit" } }) as Record<string, unknown>;
    expect("auth" in inherit).toBe(false);
    expect(inherit.body).toEqual({ mode: "graphql", graphql: {} });
  });

  test("an address that was text stays text, until a parameter is turned off", () => {
    const s = request("s");
    expect(requestToPostman({ ...s, url: "https://api.test:8443/v1/ping" })).toEqual({ url: "https://api.test:8443/v1/ping", method: "GET" });
    const off = requestToPostman(withParams({ ...s, url: "https://api.test:8443/v1/ping?a=1" }, [{ key: "a", value: "1", disabled: true }])) as { url: unknown };
    expect(off.url).toEqual({ raw: "https://api.test:8443/v1/ping", protocol: "https", host: ["api", "test"], port: "8443", path: ["v1", "ping"], query: [{ key: "a", value: "1", disabled: true }] });
  });

  test("a request made on the page is written whole", () => {
    const fresh = requestFrom("n", "New", undefined);
    expect(requestToPostman({ ...fresh, source: undefined })).toEqual({ method: "GET", url: "", header: [] });
    const made: HttpRequest = { ...fresh, source: undefined, method: "PUT", url: "/x", auth: { type: "basic", username: "u", password: "{{p}}" }, body: { mode: "urlencoded", fields: [{ key: "a", value: "1" }] } };
    expect(requestToPostman(made)).toEqual({
      method: "PUT",
      url: "/x",
      header: [],
      auth: { type: "basic", basic: [{ key: "username", value: "u", type: "string" }, { key: "password", value: "{{p}}", type: "string" }] },
      body: { mode: "urlencoded", urlencoded: [{ key: "a", value: "1" }] },
    });
  });

  test("a row keeps its extras by its key, not its place, and an address keeps what is not its parts", () => {
    const source = {
      method: "GET",
      header: [
        { key: "A", value: "1", description: "first" },
        { key: "B", value: "2", type: "text", description: "second" },
      ],
      body: { mode: "urlencoded", urlencoded: [{ key: "x", value: "1", description: "gone" }, { key: "y", value: "2", description: "kept" }] },
      url: { raw: "https://h.test/p?page=1", protocol: "https", host: ["h", "test"], path: ["p"], query: [{ key: "page", value: "1", description: "page" }], hash: "top" },
    };
    const r = requestFrom("r", "R", source);
    const out = requestToPostman({
      ...r,
      url: "{{baseUrl}}/q?page=2",
      headers: r.headers.slice(1),
      body: { mode: "urlencoded", fields: [{ key: "y", value: "3" }] },
    }) as { header: unknown; body: { urlencoded: unknown }; url: unknown };
    expect(out.header).toEqual([{ key: "B", value: "2", type: "text", description: "second" }]);
    expect(out.body.urlencoded).toEqual([{ key: "y", value: "3", description: "kept" }]);
    expect(out.url).toEqual({ raw: "{{baseUrl}}/q?page=2", host: ["{{baseUrl}}"], path: ["q"], query: [{ key: "page", value: "2", description: "page" }], hash: "top" });
  });

  test("a body in a language the page reads as text keeps its language", () => {
    const r = requestFrom("r", "R", { method: "POST", url: "/", body: { mode: "raw", raw: "<p/>", options: { raw: { language: "html" } } } });
    expect(r.body).toMatchObject({ language: "text" });
    const out = requestToPostman({ ...r, body: { mode: "raw", language: "text", raw: "<b/>" } }) as { body: unknown };
    expect(out.body).toEqual({ mode: "raw", raw: "<b/>", options: { raw: { language: "html" } } });
    const json = requestToPostman({ ...r, body: { mode: "raw", language: "json", raw: "{}" } }) as { body: unknown };
    expect(json.body).toEqual({ mode: "raw", raw: "{}", options: { raw: { language: "json" } } });
  });

  test("a collection's secret variable is masked in Preview and named in cURL", () => {
    const c = collectionFrom({
      id: "c",
      file: { info: { name: "S" }, variable: [{ key: "token", value: "pm-hunter2", type: "secret" }, { key: "id", value: "1" }], item: [{ id: "r", name: "R", request: { method: "GET", url: "/x/{{id}}", header: [{ key: "T", value: "{{token}}" }] } }] },
      error: null,
    });
    const r = findRequest([c], "r")!.request;
    expect(resolveRequest(c, r, null).headers).toContainEqual({ key: "T", value: "•••" });
    const curl = toCurl(resolveRequest(c, r, null, "name"));
    expect(curl).toContain("{{token}}");
    expect(curl).toContain("/x/1");
    expect(curl).not.toContain("pm-hunter2");
  });

  function findSource(id: string): unknown {
    const walk = (items: unknown[]): unknown =>
      items.map((i) => i as { id: string; request?: unknown; item?: unknown[] }).reduce<unknown>((hit, i) => hit ?? (i.id === id ? i.request : i.item ? walk(i.item) : undefined), undefined);
    return walk(FILE.item);
  }
});
