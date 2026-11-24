import { useRef, useState } from "react";
import { FileUp } from "lucide-react";
import { certificateCount, pemFromBytes } from "../lib/providerForm";
import "./Certificate.css";

/**
 * A certificate to trust for one server — a provider's gateway, a Bitbucket
 * behind a corporate CA, a self-signed server — and for no other. Loaded from
 * a file or pasted; shown as a count, and as text only when asked.
 */
export function Certificate({
  pem,
  source,
  onChange,
}: {
  pem: string;
  /** The built-in provider this certificate came with, when it is one of its own. */
  source?: string;
  onChange: (pem: string) => void;
}) {
  const file = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [readError, setReadError] = useState<string | null>(null);
  const count = certificateCount(pem);

  const load = async (chosen: File | undefined) => {
    if (!chosen) return;
    try {
      onChange(pemFromBytes(new Uint8Array(await chosen.arrayBuffer())));
      setReadError(null);
    } catch (e) {
      setReadError(`${chosen.name} could not be read: ${e}`);
    }
  };

  return (
    <section className="cert-card">
      <div className="cert-head">
        <h4 className="cert-title">Certificate</h4>
        <div className="cert-actions">
          <button type="button" className="btn btn-ghost" onClick={() => file.current?.click()}>
            <FileUp size={12} />
            {pem.trim() ? "Replace…" : "Load file…"}
          </button>
          <button type="button" className="btn btn-ghost" onClick={() => setOpen(!open)}>
            {open ? "Hide" : pem.trim() ? "Show" : "Paste"}
          </button>
          {pem.trim() && (
            <button type="button" className="btn btn-ghost" onClick={() => onChange("")}>
              Remove
            </button>
          )}
        </div>
        <input
          ref={file}
          type="file"
          hidden
          accept=".pem,.crt,.cer,.der"
          onChange={(e) => {
            load(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
      </div>
      <p className={`cert-state${pem.trim() ? " set" : ""}`}>
        {pem.trim()
          ? count
            ? source
              ? `${count} certificate${count === 1 ? "" : "s"} — the one ${source} comes with, trusted instead of the public roots`
              : `${count} certificate${count === 1 ? "" : "s"} — trusted instead of the public roots`
            : "No certificate found in this text"
          : "None — the server is checked against the public roots"}
      </p>
      {open && (
        <textarea
          className="cert-text"
          rows={6}
          spellCheck={false}
          value={pem}
          placeholder={"-----BEGIN CERTIFICATE-----\n…\n-----END CERTIFICATE-----"}
          onChange={(e) => onChange(e.target.value)}
        />
      )}
      {readError && <p className="modal-note cert-error">{readError}</p>}
      <p className="modal-note">
        The server's own certificate, if it is self-signed, or the CA that issued it. A chain can be pasted whole.
      </p>
    </section>
  );
}
