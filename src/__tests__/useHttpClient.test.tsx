import { beforeEach, describe, expect, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";
import { useHttpClient } from "../hooks/useHttpClient";

beforeEach(() => localStorage.clear());

describe("the HTTP page's state across a restart", () => {
  test("the open tabs and a request's last response come back", async () => {
    const first = renderHook(() => useHttpClient());
    act(() => first.result.current.open("health"));
    act(() => first.result.current.send("health"));
    await waitFor(() => expect(first.result.current.sent.health?.response?.status).toBe(200), { timeout: 2000 });
    const kept = first.result.current.sent.health;
    first.unmount();

    const again = renderHook(() => useHttpClient());
    expect(again.result.current.tabs).toContain("health");
    expect(again.result.current.active).toBe("health");
    expect(again.result.current.sent.health).toMatchObject({ busy: false, at: kept.at, environment: "local", response: kept.response });
  });

  test("a send goes into the history with its request and response, a secret masked, and the history comes back", async () => {
    const first = renderHook(() => useHttpClient());
    const before = first.result.current.history.length;
    act(() => first.result.current.setEnvironmentId("test"));
    act(() => first.result.current.send("create-order"));
    await waitFor(() => expect(first.result.current.history.length).toBe(before + 1), { timeout: 2000 });
    const entry = first.result.current.history[0];
    expect(entry.request?.url).toBe("https://orders.test.corp.example/v1/orders");
    expect(entry.request?.headers).toContainEqual({ key: "Authorization", value: "Bearer •••" });
    expect(entry.request?.body).toContain('"customerId": "cus_19"');
    expect(entry.response?.status).toBe(201);
    expect(entry.response?.body).toContain("ord_8Kp4");
    first.unmount();

    const again = renderHook(() => useHttpClient());
    expect(again.result.current.history[0]).toEqual(entry);
  });

  test("a refusal before sending is not kept, and leaves the last response in place", async () => {
    const page = renderHook(() => useHttpClient());
    act(() => page.result.current.send("health"));
    await waitFor(() => expect(page.result.current.sent.health?.response).toBeDefined(), { timeout: 2000 });
    act(() => page.result.current.setEnvironmentId("prod"));
    act(() => page.result.current.send("refund")); // {{paymentId}} is nowhere
    expect(page.result.current.sent.refund?.notSent).toContain("{{paymentId}}");
    page.unmount();

    const again = renderHook(() => useHttpClient());
    expect(again.result.current.sent.refund).toBeUndefined();
    expect(again.result.current.sent.health?.response?.status).toBe(200);
  });

  test("a new request opens in a tab; a deleted folder takes its requests' tabs and answers with it", async () => {
    const page = renderHook(() => useHttpClient());
    act(() => page.result.current.send("create-order"));
    await waitFor(() => expect(page.result.current.sent["create-order"]?.response).toBeDefined(), { timeout: 2000 });
    let id = "";
    act(() => {
      id = page.result.current.newRequest("orders");
    });
    expect(page.result.current.active).toBe(id);
    act(() => page.result.current.setActive("create-order"));
    act(() => page.result.current.remove("orders-folder"));
    expect(page.result.current.tabs).toEqual([id]);
    expect(page.result.current.active).toBe(id);
    expect(page.result.current.sent["create-order"]?.response).toBeUndefined();
    act(() => page.result.current.rename(id, "Ping"));
    expect(page.result.current.collections[0].items.map((i) => i.name)).toEqual(["Health", "Ping"]);
  });
});

