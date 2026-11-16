import { useState } from "react";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { Modal } from "./Modal";
import { CodeHostConnect } from "./CodeHostConnect";
import { useCodeHosts } from "../hooks/useCodeHosts";
import { certificateCount } from "../lib/providerForm";
import type { CodeHostServer } from "../lib/pullRequests";
import "./CodeHostSettings.css";

const NEW: CodeHostServer = { id: "", baseUrl: "", hosts: [], trustedCertPem: null, tokenSet: false };

/**
 * Settings → Bitbucket: the servers the pull request strip reads from, each
 * with its address, the hosts learnt from its clone links, its certificate and
 * whether a token is kept. The token goes in and never comes back.
 */
export function CodeHostSettings() {
  const { servers, error, save, remove, detect } = useCodeHosts();
  const [editing, setEditing] = useState<CodeHostServer | null>(null);
  const [deleting, setDeleting] = useState<CodeHostServer | null>(null);

  return (
    <>
      <h3 className="settings-title">Bitbucket</h3>
      <p className="modal-note code-hosts-intro">
        The Bitbucket Data Center servers the pull request above the Changes list is read from. A folder finds its server by
        its remote: by the address, or by a host the server's clone links have shown it answers to.
      </p>
      {error && <p className="modal-note code-hosts-error">{error}</p>}
      <ul className="code-hosts" aria-label="Bitbucket servers">
        {servers.map((server) => {
          const certs = certificateCount(server.trustedCertPem ?? "");
          return (
            <li key={server.id} className="code-host">
              <div className="code-host-text">
                <span className="code-host-url">{server.baseUrl}</span>
                <span className="code-host-meta">
                  {[
                    server.tokenSet ? "token kept" : "no token",
                    server.hosts.length > 0 && `also ${server.hosts.join(", ")}`,
                    certs > 0 && `${certs} certificate${certs === 1 ? "" : "s"}`,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              </div>
              <button type="button" className="code-host-icon" aria-label={`Edit ${server.baseUrl}`} onClick={() => setEditing(server)}>
                <Pencil size={12} />
              </button>
              <button type="button" className="code-host-icon" aria-label={`Delete ${server.baseUrl}`} onClick={() => setDeleting(server)}>
                <Trash2 size={12} />
              </button>
            </li>
          );
        })}
        {servers.length === 0 && <li className="code-host-empty">None yet. A folder whose remote no server answers to offers to connect one.</li>}
      </ul>
      <button type="button" className="btn btn-ghost" onClick={() => setEditing(NEW)}>
        <Plus size={12} /> Add server
      </button>

      <CodeHostConnect
        open={editing !== null}
        server={editing ?? NEW}
        forFolder={false}
        onClose={() => setEditing(null)}
        onSave={save}
        onDetect={detect}
      />
      <Modal
        title="Delete server"
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setDeleting(null)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                if (deleting) void remove(deleting.id);
                setDeleting(null);
              }}
            >
              Delete
            </button>
          </>
        }
      >
        <p className="modal-note">
          Forget {deleting?.baseUrl} and its token? Its folders show "not connected" until it is added again.
        </p>
      </Modal>
    </>
  );
}
