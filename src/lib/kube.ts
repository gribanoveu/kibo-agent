import { invoke } from "@tauri-apps/api/core";

/** A kubeconfig file Chat mode's Kubernetes role can be pointed at — by path, never contents. */
export type Kubeconfig = {
  name: string;
  path: string;
  /** The user's mark: every change to its clusters asks, and the card says where it lands. */
  production?: boolean;
};
export type KubeSettings = { configs: Kubeconfig[]; active: string | null };

export async function kubeSettings(): Promise<KubeSettings> {
  return invoke<KubeSettings>("kube_settings_get");
}

/** Adds it, or replaces the one of that name. Refused unless a file is at `path`; `~` is the home folder. */
export async function saveKubeconfig(name: string, path: string): Promise<void> {
  await invoke("kubeconfig_save", { name, path });
}

export async function setKubeconfigProduction(name: string, production: boolean): Promise<void> {
  await invoke("kubeconfig_production_set", { name, production });
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

/** Mirrors `domain::kube::KubePin`: where a chat works. `null` picks what `kubectl` would. */
export type KubePin = {
  kubeconfig: string;
  context: string | null;
  namespace: string | null;
  /** "Changes" on the chat's tab: its tools may change the cluster. Absent is read-only. */
  writes?: boolean;
};
export type KubeContext = { name: string; cluster: string; namespace: string | null };
export type KubeContexts = { contexts: KubeContext[]; current: string | null };
/** The namespaces a chat's menu offers, by where each came from. */
export type KubeNamespaces = { kubeconfig: string[]; typed: string[]; cluster: string[]; clusterError: string | null };

/** A kubeconfig's contexts, from the file alone. */
export async function kubeContexts(kubeconfig: string): Promise<KubeContexts> {
  return invoke<KubeContexts>("kube_contexts", { kubeconfig });
}

/** The file's namespaces, the typed ones, and the cluster's when this identity may list them. */
export async function kubeNamespaces(pin: KubePin): Promise<KubeNamespaces> {
  return invoke<KubeNamespaces>("kube_namespaces", { pin });
}

/** A namespace typed on a chat's tab, offered again for that kubeconfig. */
export async function rememberNamespace(kubeconfig: string, namespace: string): Promise<void> {
  await invoke("kube_namespace_remember", { kubeconfig, namespace });
}

/** The context a pin names, or the file's current one — as the backend reads it. */
export function chosenContext(contexts: KubeContexts | null, pin: KubePin | null): KubeContext | null {
  if (!contexts || !pin) return null;
  const name = pin.context ?? contexts.current;
  return contexts.contexts.find((c) => c.name === name) ?? null;
}

/** The namespace a pin names, or its context's, or `default`. */
export function chosenNamespace(context: KubeContext | null, pin: KubePin | null): string {
  return pin?.namespace ?? context?.namespace ?? "default";
}

/** Mirrors `domain::kube::KubeChange`. */
export type KubeChange = {
  id: string;
  at: string;
  kubeconfig: string;
  context: string;
  namespace: string;
  kind: string;
  name: string;
  tool: string;
  summary: string;
  error: string | null;
  /** The change this one put back, when it is an undo. */
  undoes: string | null;
};

/** What the app changed in the user's clusters and can still put back, newest first. */
export async function kubeChanges(): Promise<KubeChange[]> {
  return invoke<KubeChange[]>("kube_changes");
}
