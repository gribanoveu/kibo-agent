import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { Modal } from "./Modal";
import { Certificate } from "./Certificate";
import type { CodeHostServer, FetchedCert } from "../lib/pullRequests";
import "./CodeHostConnect.css";

type Props = {
  open: boolean;
  /** The server edited, or a new one filled from the folder's remote (`id` empty). */
  server: CodeHostServer;
  /** From the strip: the open folder's repository is proven to be on the server before it is kept. */
  forFolder: boolean;
  onClose: () => void;
  /** `token` of `null` keeps the saved one. */
  onSave: (server: CodeHostServer, token: string | null) => Promise<unknown>;
  /** What the address answers as, asked without a token. */
  onDetect: (server: CodeHostServer) => Promise<string>;
  /** The certificate the address presents, taken without a token (BB-10). */
  onFetch: (baseUrl: string) => Promise<FetchedCert>;
};

/**
 * Adds or edits a Bitbucket Data Center server: its address, the certificate
 * it is trusted by (before Check, which goes through it), and an HTTP access
 * token. Check asks the address what it is, without the token.
 */
export function CodeHostConnect({ open, server, forFolder, onClose, onSave, onDetect, onFetch }: Props) {
  const [baseUrl, setBaseUrl] = useState(server.baseUrl);
  const [cert, setCert] = useState(server.trustedCertPem ?? "");
  const [token, setToken] = useState("");
  const [detected, setDetected] = useState<{ ok: boolean; text: string } | null>(null);
  const [fetched, setFetched] = useState<FetchedCert | null>(null);
  const [busy, setBusy] = useState<"check" | "save" | "fetch" | null>(null);
  const [error, setError] = useState<string | null>(null);

  // A form opened again starts from the server it is opened for.
  useEffect(() => {
    if (!open) return;
    setBaseUrl(server.baseUrl);
    setCert(server.trustedCertPem ?? "");
    setToken("");
    setDetected(null);
    setFetched(null);
    setError(null);
    // Only on opening: a parent's new object for the same server must not wipe what is being typed.
  }, [open]);

  const draft = (): CodeHostServer => ({ ...server, baseUrl: baseUrl.trim(), trustedCertPem: cert.trim() ? cert : null });

  const check = async () => {
    setBusy("check");
    setFetched(null);
    try {
      setDetected({ ok: true, text: await onDetect(draft()) });
    } catch (e) {
      setDetected({ ok: false, text: String(e) });
    } finally {
      setBusy(null);
    }
  };

  const fetch = async () => {
    setBusy("fetch");
    try {
      setFetched(await onFetch(baseUrl.trim()));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const save = async () => {
    setBusy("save");
    try {
      await onSave(draft(), token.trim() ? token : null);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const tokenNeeded = !server.tokenSet && !token.trim();
  return (
    <Modal
      title={server.id ? "Bitbucket server" : "Connect Bitbucket"}
      open={open}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" disabled={!baseUrl.trim() || tokenNeeded || busy !== null} onClick={save}>
            {busy === "save" && <Loader2 className="code-host-spin" size={12} />}
            {forFolder ? "Connect" : "Save"}
          </button>
        </>
      }
    >
      <div className="modal-field">
        <label htmlFor="code-host-url">Address</label>
        <div className="code-host-row">
          <input
            id="code-host-url"
            value={baseUrl}
            placeholder="https://bitbucket.example.com"
            spellCheck={false}
            onChange={(e) => {
              setBaseUrl(e.target.value);
              setDetected(null);
              // A certificate belongs to the address it was fetched from: left
              // here, it could be accepted for another host.
              setFetched(null);
            }}
          />
          <button type="button" className="btn btn-ghost" disabled={!baseUrl.trim() || busy !== null} onClick={check}>
            {busy === "check" && <Loader2 className="code-host-spin" size={12} />}
            Check
          </button>
        </div>
        {detected && <p className={`modal-note${detected.ok ? " code-host-ok" : " code-host-error"}`}>{detected.text}</p>}
        {/* Only where Check failed: a certificate is what a refused handshake is about. */}
        {detected && !detected.ok && !fetched && (
          <button type="button" className="btn btn-ghost" disabled={busy !== null} onClick={fetch}>
            {busy === "fetch" && <Loader2 className="code-host-spin" size={12} />}
            Fetch certificate
          </button>
        )}
        {fetched && (
          <div className="code-host-fetched">
            <p className="modal-note">
              {fetched.chain > 1
                ? `The server sent ${fetched.chain} certificates; this is the one above the rest.`
                : "The server's own certificate."}{" "}
              SHA-256:
            </p>
            <code className="code-host-fingerprint">{fetched.fingerprint}</code>
            <p className="modal-note">
              It came over the connection you do not trust yet, so anyone on the path could have shown it. Compare it with
              the fingerprint your IT gives you, or with `openssl s_client -showcerts -connect host:443`, before trusting it.
            </p>
            <div className="code-host-fetched-actions">
              <button type="button" className="btn btn-ghost" onClick={() => setFetched(null)}>
                Discard
              </button>
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => {
                  // Into the field, not into the saved server: Check and Save are still the user's.
                  setCert(fetched.pem);
                  setFetched(null);
                  setDetected(null);
                }}
              >
                Use this certificate
              </button>
            </div>
          </div>
        )}
      </div>

      <Certificate pem={cert} onChange={setCert} />

      <div className="modal-field code-host-token">
        <label htmlFor="code-host-token">HTTP access token</label>
        <input
          id="code-host-token"
          type="password"
          value={token}
          placeholder={server.tokenSet ? "Kept — type to replace" : "Repository read; write for Approve, Merge and Create"}
          autoComplete="off"
          onChange={(e) => setToken(e.target.value)}
        />
        <p className="modal-note">
          Sealed on this computer as the API keys are, and sent to this server only.
          {forFolder && " Connect checks that this folder's repository is on it."}
        </p>
      </div>
      {error && <p className="modal-note code-host-error">{error}</p>}
    </Modal>
  );
}
