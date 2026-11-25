import { useEffect, useRef, useState } from "react";
import { Loader2, ShieldAlert } from "lucide-react";
import { Modal } from "./Modal";
import { Certificate } from "./Certificate";
import { CopyAction } from "./CopyAction";
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

const NEW_HOST = /^(https?:\/\/)?([^/]+)/;

/** The host of what is typed, for the block's heading; the text itself when it is not an address yet. */
const hostOf = (url: string) => url.trim().match(NEW_HOST)?.[2] ?? url.trim();

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
  // The address the answer being awaited was asked about. The field stays
  // editable while a check or a handshake runs, and an answer about another
  // host is not this one's — a certificate least of all: taking it would trust
  // one host's key for another's.
  const asked = useRef<string | null>(null);

  // A form opened again starts from the server it is opened for.
  useEffect(() => {
    if (!open) return;
    setBaseUrl(server.baseUrl);
    setCert(server.trustedCertPem ?? "");
    setToken("");
    setDetected(null);
    setFetched(null);
    setError(null);
    asked.current = null;
    // Only on opening: a parent's new object for the same server must not wipe what is being typed.
  }, [open]);

  const draft = (): CodeHostServer => ({ ...server, baseUrl: baseUrl.trim(), trustedCertPem: cert.trim() ? cert : null });

  const check = async () => {
    const address = baseUrl.trim();
    asked.current = address;
    setBusy("check");
    setFetched(null);
    try {
      const text = await onDetect(draft());
      if (asked.current === address) setDetected({ ok: true, text });
    } catch (e) {
      if (asked.current === address) setDetected({ ok: false, text: String(e) });
    } finally {
      setBusy(null);
    }
  };

  const fetch = async () => {
    const address = baseUrl.trim();
    asked.current = address;
    setBusy("fetch");
    try {
      const certificate = await onFetch(address);
      // The address was edited while the handshake ran: this certificate is not
      // the one the field now names.
      if (asked.current !== address) return;
      setFetched(certificate);
      setError(null);
    } catch (e) {
      if (asked.current === address) setError(String(e));
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
              // here, it could be accepted for another host. An answer still on
              // its way is dropped the same way, by no longer being awaited for
              // this text.
              setFetched(null);
              asked.current = null;
            }}
          />
          <button type="button" className="btn btn-ghost" disabled={!baseUrl.trim() || busy !== null} onClick={check}>
            {busy === "check" && <Loader2 className="code-host-spin" size={12} />}
            Check
          </button>
        </div>
        {detected && (
          <p className={`modal-note${detected.ok ? " code-host-ok" : " code-host-error"}`} title={detected.text}>
            {/* The refusal names the Catalog panel the user is already in, and rustls'
                own words; the short line is what they need, the whole text a hover away. */}
            {!detected.ok && detected.text.includes("certificate is not trusted")
              ? "The server's certificate is not trusted yet."
              : detected.text}
          </p>
        )}
        {/* Only where Check failed: a certificate is what a refused handshake is about. */}
        {detected && !detected.ok && !fetched && (
          <button type="button" className="btn btn-ghost" disabled={busy !== null} onClick={fetch}>
            {busy === "fetch" && <Loader2 className="code-host-spin" size={12} />}
            Fetch certificate
          </button>
        )}
        {fetched && (
          <div className="code-host-fetched">
            <h4 className="code-host-fetched-title">Certificate from {hostOf(baseUrl)}</h4>
            <dl className="code-host-facts">
              <dt>Issued to</dt>
              <dd>{fetched.subject}</dd>
              <dt>Issued by</dt>
              <dd>
                {fetched.issuer}
                {fetched.selfSigned && <span className="code-host-badge">Self-signed</span>}
              </dd>
              <dt>Valid until</dt>
              <dd className={fetched.expired || fetched.notYetValid ? "code-host-error" : undefined}>
                {fetched.validUntil}
                {fetched.expired && " — expired"}
                {fetched.notYetValid && " — not valid yet"}
                
              </dd>
              <dt>SHA-256</dt>
              <dd className="code-host-fingerprint">
                <code>{fetched.fingerprint}</code>
                <CopyAction text={fetched.fingerprint} />
              </dd>
            </dl>
            <p
              className="modal-note code-host-caution"
              title="openssl s_client -showcerts -connect host:443"
            >
              <ShieldAlert size={14} /> Shown over a connection you do not trust yet — compare the fingerprint with your IT's.
              {fetched.chain > 1 &&
                (fetched.selfSigned
                  ? ` The server sent ${fetched.chain} certificates; this is its root.`
                  : ` The server sent ${fetched.chain} certificates and none is a root; this is the last one it sent.`)}
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
