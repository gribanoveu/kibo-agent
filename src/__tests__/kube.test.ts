import { describe, expect, test } from "bun:test";
import { activeKubeconfig } from "../lib/kube";

const prod = { name: "prod", path: "/k/prod" };
const staging = { name: "staging", path: "/k/staging" };

describe("the kubeconfig in use", () => {
  test("is the first while none is picked", () => {
    expect(activeKubeconfig({ configs: [prod, staging], active: null })).toEqual(prod);
    expect(activeKubeconfig({ configs: [], active: null })).toBeNull();
    expect(activeKubeconfig(null)).toBeNull();
  });

  test("is the picked one, and none when the pick was removed", () => {
    expect(activeKubeconfig({ configs: [prod, staging], active: "staging" })).toEqual(staging);
    expect(activeKubeconfig({ configs: [prod], active: "staging" })).toBeNull();
  });
});
