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
});
