import { describe, expect, test } from "bun:test";
import { activeKubeconfig, chosenContext, chosenNamespace } from "../lib/kube";
import { namespaceOptions } from "../hooks/useKubeTarget";

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

describe("a chat's pin", () => {
  const contexts = {
    contexts: [
      { name: "prod", cluster: "eks", namespace: "payments" },
      { name: "stg", cluster: "stg", namespace: null },
    ],
    current: "prod",
  };
  const pin = { kubeconfig: "k", context: null, namespace: null };

  test("with nothing chosen is the current context and its namespace, as kubectl takes them", () => {
    const context = chosenContext(contexts, pin);
    expect(context?.name).toBe("prod");
    expect(chosenNamespace(context, pin)).toBe("payments");
  });

  test("a context without a namespace is in default; a chosen one wins", () => {
    const staging = { ...pin, context: "stg" };
    expect(chosenNamespace(chosenContext(contexts, staging), staging)).toBe("default");
    const orders = { ...staging, namespace: "orders" };
    expect(chosenNamespace(chosenContext(contexts, orders), orders)).toBe("orders");
    expect(chosenContext(contexts, { ...pin, context: "gone" })).toBeNull();
  });

  test("the namespace menu has the one in use first, then each other once, saying where it came from", () => {
    const options = namespaceOptions("payments", {
      kubeconfig: ["payments", "billing"],
      typed: ["orders", "billing"],
      cluster: ["default", "orders", "kube-system"],
      clusterError: null,
    });
    expect(options).toEqual([
      { value: "payments" },
      { value: "billing", hint: "from the kubeconfig" },
      { value: "orders", hint: "typed before" },
      { value: "default", hint: "in the cluster" },
      { value: "kube-system", hint: "in the cluster" },
    ]);
    expect(namespaceOptions("default", null)).toEqual([{ value: "default" }]);
  });
});
