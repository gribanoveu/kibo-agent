import { describe, expect, test } from "bun:test";
import { pmCompletions, pmVariableAt } from "../lib/pmCompletions";

const names = (before: string, listen: "prerequest" | "test" = "test") => pmCompletions(before, listen)?.members.map((m) => m.name) ?? null;

describe("what the script editor offers", () => {
  test("pm's members after `pm.`, and a scope's after one of them", () => {
    expect(names("pm.")).toContain("environment");
    expect(names("pm.")).toContain("sendRequest");
    expect(names("const id = pm.environment.")).toEqual(["get", "set", "unset", "has", "clear", "toObject", "replaceIn"]);
    expect(names("pm.collectionVariables.se")).toContain("set");
  });

  test("where the name being typed starts, so a pick replaces it", () => {
    expect(pmCompletions("x = pm.environment.ge", "test")?.from).toBe("x = pm.environment.".length);
    expect(pmCompletions("pm.", "test")?.from).toBe(3);
  });

  test("a call in the chain is read past, whatever its arguments", () => {
    expect(names("pm.expect(pm.response.json().items.length).to.")).toContain("equal");
    expect(names("pm.expect(a(b(c))).to.be.")).toContain("above");
    expect(names("pm.response.to.have.")).toEqual(["status", "header", "body", "jsonBody", "jsonSchema"]);
    expect(names('pm.test("ok", () => {\n  pm.response.')).toContain("json");
  });

  test("the response only where there is one; skipping the request only before it goes", () => {
    expect(names("pm.", "prerequest")).not.toContain("response");
    expect(names("pm.", "test")).toContain("response");
    expect(names("pm.execution.", "prerequest")).toContain("skipRequest");
    expect(names("pm.execution.", "test")).not.toContain("skipRequest");
  });

  test("a name at the start once something is typed; nothing for what the API does not know", () => {
    expect(names("p")).toEqual(["pm", "console", "require", "postman"]);
    expect(names("")).toBeNull();
    expect(names("foo.")).toBeNull();
    expect(names("pm.nothing.")).toBeNull();
    expect(names("pm.info.requestName.")).toBeNull();
  });
});

describe("a variable's name in pm's get and set", () => {
  test("which scope, and where the name starts, in either quote", () => {
    expect(pmVariableAt("const t = pm.environment.get('to")).toEqual({ scope: "environment", from: "const t = pm.environment.get('".length });
    expect(pmVariableAt('pm.collectionVariables.set("')).toEqual({ scope: "collectionVariables", from: 'pm.collectionVariables.set("'.length });
    expect(pmVariableAt("pm.globals.unset( `a")?.scope).toBe("globals");
    expect(pmVariableAt("pm.variables.has('x")?.scope).toBe("variables");
  });

  test("nothing once the string is closed, or for a method that takes no name", () => {
    expect(pmVariableAt("pm.environment.get('token')")).toBeNull();
    expect(pmVariableAt("pm.environment.get('token', ")).toBeNull();
    expect(pmVariableAt("pm.environment.replaceIn('{{")).toBeNull();
    expect(pmVariableAt("pm.response.json('")).toBeNull();
  });
});
