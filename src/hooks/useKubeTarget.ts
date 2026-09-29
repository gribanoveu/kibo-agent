import { useCallback, useEffect, useState } from "react";
import {
  chosenContext,
  chosenNamespace,
  kubeContexts,
  kubeNamespaces,
  type KubeContexts,
  type KubeNamespaces,
  type KubePin,
} from "../lib/kube";

/**
 * What a chat's pin resolves to — its context and namespace, named — and what
 * their menus offer. Contexts come from the file as the kubeconfig changes;
 * namespaces are asked for as their menu opens, the cluster being a live call.
 */
export function useKubeTarget(pin: KubePin | null) {
  const [contexts, setContexts] = useState<KubeContexts | null>(null);
  const [namespaces, setNamespaces] = useState<KubeNamespaces | null>(null);
  const [error, setError] = useState<string | null>(null);
  const kubeconfig = pin?.kubeconfig ?? null;

  useEffect(() => {
    setContexts(null);
    setNamespaces(null);
    setError(null);
    if (!kubeconfig) return;
    let live = true;
    kubeContexts(kubeconfig).then(
      (read) => live && setContexts(read),
      (e) => live && setError(String(e)),
    );
    return () => {
      live = false;
    };
  }, [kubeconfig]);

  const loadNamespaces = useCallback(() => {
    if (!pin) return;
    setNamespaces(null);
    kubeNamespaces(pin).then(setNamespaces, (e) => setError(String(e)));
  }, [pin?.kubeconfig, pin?.context]);

  const context = chosenContext(contexts, pin);
  return { contexts, context, namespace: chosenNamespace(context, pin), namespaces, loadNamespaces, error };
}

/**
 * The namespace menu: the one in use first, then the file's, the typed ones
 * and the cluster's — each once, labelled with where it was first found.
 */
export function namespaceOptions(current: string, listed: KubeNamespaces | null): { value: string; hint?: string }[] {
  const seen = new Set<string>([current]);
  const options: { value: string; hint?: string }[] = [{ value: current }];
  const add = (names: string[], hint: string) =>
    names.filter((name) => !seen.has(name) && seen.add(name)).forEach((value) => options.push({ value, hint }));
  add(listed?.kubeconfig ?? [], "from the kubeconfig");
  add(listed?.typed ?? [], "typed before");
  add(listed?.cluster ?? [], "in the cluster");
  return options;
}
