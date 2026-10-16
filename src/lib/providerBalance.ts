import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./backend";

/** `domain::provider_balance::ProviderBalance`: what is left on a provider's account. */
export type ProviderBalance = {
  /** The provider's word on whether it still covers a request. */
  available: boolean;
  /** One per currency; amounts as the provider writes them, `"110.00"`. */
  amounts: { currency: string; total: string; granted: string; toppedUp: string }[];
};

/** `null` while the feature is off or the provider does not say. */
export async function providerBalance(id: string): Promise<ProviderBalance | null> {
  if (!inTauri()) return null;
  return invoke<ProviderBalance | null>("provider_balance", { id });
}

const SIGNS: Record<string, string> = { CNY: "¥", USD: "$" };

/** `¥110.00 · $2.50` — the chip. A currency with nothing on it is left
    out (the title still has it), unless every one is empty. */
export function balanceText(balance: ProviderBalance): string {
  const held = balance.amounts.filter((a) => Number(a.total) !== 0);
  return (held.length > 0 ? held : balance.amounts.slice(0, 1))
    .map((a) => (SIGNS[a.currency] ? `${SIGNS[a.currency]}${a.total}` : `${a.total} ${a.currency}`))
    .join(" · ");
}

/** The chip's title: each currency in full, and why it is red. */
export function balanceTitle(balance: ProviderBalance): string {
  const lines = balance.amounts.map((a) => `${a.total} ${a.currency} — ${a.granted} granted, ${a.toppedUp} topped up`);
  if (!balance.available) lines.push("Not enough for a request — top up the account");
  return ["Balance", ...lines].join("\n");
}
