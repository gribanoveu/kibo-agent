import { useState } from "react";
import { FolderOpen, Server, Trash2 } from "lucide-react";
import { pickFile } from "../lib/dialog";
import type { KubeconfigsState } from "../hooks/useKubeconfigs";
import "./KubeSettings.css";

/** Settings → Kubernetes: the kubeconfig files Chat mode's Kubernetes role knows of. */
export function KubeSettings({ kube }: { kube: KubeconfigsState }) {
  const [name, setName] = useState("");
  const [path, setPath] = useState("");

  const add = async () => {
    if (await kube.save(name, path)) {
      setName("");
      setPath("");
    }
  };

  const browse = async () => {
    const chosen = await pickFile("Choose a kubeconfig");
    if (!chosen) return;
    setPath(chosen);
    // The file's own name is a fair first guess at the cluster's.
    if (!name.trim()) setName(chosen.split(/[\\/]/).pop()?.replace(/\.(ya?ml|conf)$/i, "") ?? "");
  };

  return (
    <>
      <h3 className="settings-title">Kubernetes</h3>
      <p className="modal-note kube-intro">
        The kubeconfig files the Kubernetes role in Chat mode works with; pick one on the tab above the chat's message
        box. Kibo keeps only the path — the file stays where kubectl reads it, credentials and all.
      </p>

      {kube.configs.length > 0 && (
        <ul className="kube-list">
          {kube.configs.map((config) => (
            <li key={config.name} className="kube-row">
              <Server size={14} className="kube-row-icon" />
              <span className="kube-row-text">
                <span className="kube-row-name">
                  {config.name}
                  {kube.active?.name === config.name && <span className="kube-row-badge">in use</span>}
                </span>
                <span className="kube-row-path">{config.path}</span>
              </span>
              <button
                type="button"
                className="iconbtn"
                title={`Remove ${config.name}`}
                aria-label={`Remove ${config.name}`}
                onClick={() => kube.remove(config.name)}
              >
                <Trash2 size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}

      <form
        className="kube-add"
        noValidate
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
      >
        <div className="modal-field">
          <label htmlFor="kube-path">Kubeconfig file</label>
          <div className="kube-path">
            <input
              id="kube-path"
              type="text"
              value={path}
              placeholder="~/.kube/config"
              onChange={(e) => setPath(e.target.value)}
            />
            <button type="button" className="btn btn-ghost" onClick={browse}>
              <FolderOpen size={13} />
              Browse
            </button>
          </div>
        </div>
        <div className="modal-field">
          <label htmlFor="kube-name">Name</label>
          <input id="kube-name" type="text" value={name} placeholder="prod" onChange={(e) => setName(e.target.value)} />
        </div>
        {kube.error && <p className="modal-note settings-hint settings-error">{kube.error}</p>}
        <div className="kube-actions">
          <button type="submit" className="btn btn-primary" disabled={!name.trim() || !path.trim()}>
            Add kubeconfig
          </button>
        </div>
      </form>
    </>
  );
}
