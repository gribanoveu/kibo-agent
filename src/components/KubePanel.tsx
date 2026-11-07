import { FileCog, Layers, PencilLine, Server } from "lucide-react";
import { ComposerPanel, PanelLine, PanelSwitch, panelRow } from "./ComposerSettings";
import { Dropdown } from "./Dropdown";
import type { KubeconfigsState } from "../hooks/useKubeconfigs";
import { namespaceOptions, useKubeTarget } from "../hooks/useKubeTarget";
import { rememberNamespace, type KubePin } from "../lib/kube";
import "./KubePanel.css";

// Not a kubeconfig's name: the menu's last row, which opens Settings instead.
const MANAGE_KUBECONFIGS = "\u0000manage";

type Props = {
  /** The chat's own cluster, as it is pinned. */
  pin: KubePin | null;
  onPin: (pin: KubePin) => void;
  kube: KubeconfigsState;
  /** Opens Settings → Kubernetes. */
  onSetUpKube: () => void;
};

/**
 * Where a Kubernetes chat works and whether it may change anything: said on the
 * line under the box — a cluster marked production in red, a chat that may
 * change it in amber, both read without opening anything — and set in a panel.
 */
export function KubePanel({ pin, onPin, kube, onSetUpKube }: Props) {
  // The chat's own cluster: its kubeconfig from Settings, and what its pin resolves to.
  const config = kube.configs.find((c) => c.name === pin?.kubeconfig) ?? null;
  const target = useKubeTarget(config ? pin : null);
  const known = config && pin && target.context ? pin : null;

  return (
    <ComposerPanel
      left
      title="Kubeconfig, context, namespace and access"
      className="kube-panel"
      summary={
        <>
          <span className={`composer-settings-shrink${config ? "" : " kube-panel-unset"}${config?.production ? " kube-panel-production" : ""}`}>
            {config?.name ?? "No kubeconfig"}
          </span>
          {known && <span className="composer-settings-shrink">{target.namespace}</span>}
          {known?.writes && <span className="kube-panel-changes">changes</span>}
        </>
      }
    >
      <Dropdown
        right
        title="The kubeconfig this chat works with"
        heading="Kubeconfig"
        label={panelRow(
          <FileCog size={14} />,
          "Kubeconfig",
          <span className={`composer-settings-model${config?.production ? " kube-panel-production" : ""}`}>{config?.name ?? "None"}</span>,
        )}
        value={config?.name ?? ""}
        options={[
          ...kube.configs.map((c) => ({ value: c.name, hint: c.path })),
          { value: MANAGE_KUBECONFIGS, label: kube.configs.length ? "Manage kubeconfigs…" : "Add a kubeconfig…" },
        ]}
        onPick={(value) => {
          if (value === MANAGE_KUBECONFIGS) return onSetUpKube();
          onPin({ kubeconfig: value, context: null, namespace: null });
          // Where the next new chat starts.
          void kube.pick(value);
        }}
      />
      {config && pin && (
        <Dropdown
          right
          title="The context of the kubeconfig — which cluster, as which user"
          heading="Context"
          label={panelRow(
            <Server size={14} />,
            "Context",
            <span className="composer-settings-model">{target.context?.name ?? pin.context ?? "None"}</span>,
          )}
          value={target.context?.name ?? ""}
          options={(target.contexts?.contexts ?? []).map((c) => ({
            value: c.name,
            hint: [c.cluster, c.namespace, c.name === target.contexts?.current ? "current" : null].filter(Boolean).join(" · "),
          }))}
          emptyLabel={target.error ?? "The kubeconfig has no contexts"}
          onPick={(context) => onPin({ ...pin, context, namespace: null, writes: false })}
        />
      )}
      {known && (
        <Dropdown
          right
          title="The namespace this chat works in"
          heading="Namespace"
          label={panelRow(<Layers size={14} />, "Namespace", <span className="composer-settings-model">{target.namespace}</span>)}
          value={target.namespace}
          options={namespaceOptions(target.namespace, target.namespaces)}
          onOpen={target.loadNamespaces}
          note={
            target.namespaces === null
              ? "Asking the cluster…"
              : target.namespaces.clusterError
                ? `The cluster did not list its namespaces: ${target.namespaces.clusterError}`
                : undefined
          }
          custom={{
            placeholder: "Another namespace",
            onEnter: (namespace) => {
              onPin({ ...known, namespace, writes: false });
              void rememberNamespace(known.kubeconfig, namespace).catch(() => {});
            },
          }}
          onPick={(namespace) => onPin({ ...known, namespace, writes: false })}
        />
      )}
      {known && (
        <>
          <PanelLine />
          <PanelSwitch
            label="Allow changes"
            icon={<PencilLine size={14} />}
            title={`Off, the model only reads the cluster. On, it may change ${target.namespace} — each change asks you first and is backed up`}
            on={known.writes ?? false}
            onChange={(writes) => onPin({ ...known, writes })}
            warn
          />
        </>
      )}
    </ComposerPanel>
  );
}
