import { invoke } from "@tauri-apps/api/core";

/** A kubeconfig file Chat mode's Kubernetes role can be pointed at — by path, never contents. */
export type Kubeconfig = { name: string; path: string };
export type KubeSettings = { configs: Kubeconfig[]; active: string | null };

export async function kubeSettings(): Promise<KubeSettings> {
  return invoke<KubeSettings>("kube_settings_get");
}

/** Adds it, or replaces the one of that name. Refused unless a file is at `path`; `~` is the home folder. */
export async function saveKubeconfig(name: string, path: string): Promise<void> {
  await invoke("kubeconfig_save", { name, path });
}

export async function removeKubeconfig(name: string): Promise<void> {
  await invoke("kubeconfig_remove", { name });
}

/** The one the role works with; `null` goes back to the first. */
export async function pickKubeconfig(name: string | null): Promise<void> {
  await invoke("kubeconfig_pick", { name });
}

/** The one in use, as the backend reads it: the picked one, or the first when none is picked. */
export function activeKubeconfig(settings: KubeSettings | null): Kubeconfig | null {
  if (!settings) return null;
  if (settings.active === null) return settings.configs[0] ?? null;
  return settings.configs.find((c) => c.name === settings.active) ?? null;
}
