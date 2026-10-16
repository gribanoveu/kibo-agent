import { Wallet } from "lucide-react";
import { useProviderBalance } from "../hooks/useProviderBalance";
import { balanceText, balanceTitle } from "../lib/providerBalance";
import "./ProviderBalance.css";

/** What is left on the account of the provider turns go to — nothing while
    the Catalog's "Provider balance" is off or the provider does not say. */
export function ProviderBalance({ providerId, running }: { providerId: string | null; running: boolean }) {
  const { balance, error } = useProviderBalance(providerId, running);
  if (error) {
    return (
      <span className="provider-balance unknown" title={`Balance: ${error}`}>
        <Wallet size={12} />—
      </span>
    );
  }
  if (!balance || balance.amounts.length === 0) return null;
  return (
    <span className={`provider-balance${balance.available ? "" : " spent"}`} title={balanceTitle(balance)}>
      <Wallet size={12} />
      {balanceText(balance)}
    </span>
  );
}
