import { ChevronDown, ChevronRight, File, MessageSquarePlus, Plus, Trash2 } from "lucide-react";
import type { PrototypePreview as Preview } from "../hooks/usePrototypePreview";
import { pickImages } from "../lib/dialog";
import { assetUrl, sizeLabel, type Asset } from "../lib/prototype";
import "./PrototypeAssets.css";

type Props = {
  preview: Preview;
  chatId: string;
  onNotify: (message: string) => void;
};

/**
 * The drawer under the page in the Preview panel: the pictures in the chat's
 * `assets/`, which a page names as `assets/<name>` (docs/31-prototype-role.md,
 * P-10). Closed until asked for, or until pictures are attached to a message.
 * A picture dropped on the window goes to the message box, as anywhere else in
 * the chat; here it is added from the system's picker.
 */
export function PrototypeAssets({ preview, chatId, onNotify }: Props) {
  const assets = preview.assets?.assets ?? [];
  const folder = preview.assets;

  const add = async () => {
    const paths = await pickImages();
    if (paths.length === 0) return;
    const failed = await preview.addAssets(paths);
    if (failed.length > 0) onNotify(failed.join("\n"));
  };

  const remove = (asset: Asset) => preview.removeAsset(asset.name).catch((e) => onNotify(String(e)));

  return (
    <section className="prototype-assets">
      <button type="button" className="prototype-assets-head" aria-expanded={preview.assetsOpen} onClick={preview.toggleAssets}>
        {preview.assetsOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <span className="prototype-assets-title">Assets</span>
        {folder && (
          <span className="prototype-assets-meta" title="The chat's folder against the most a version keeps">
            {assets.length} · {sizeLabel(folder.folderBytes)} of {sizeLabel(folder.limitBytes)}
          </span>
        )}
      </button>
      {preview.assetsOpen && (
        <ul className="prototype-assets-grid">
          {assets.map((asset) => (
            <li key={asset.name} className="prototype-assets-item">
              <div className="prototype-assets-thumb">
                {asset.width !== null ? (
                  // The size in the address: a name used again after a delete is another picture.
                  <img src={`${assetUrl(chatId, asset.name)}?${asset.bytes}`} alt="" loading="lazy" />
                ) : (
                  <File size={18} />
                )}
                <div className="prototype-assets-actions">
                  <button
                    type="button"
                    className="iconbtn"
                    title="Put its name in the message"
                    onClick={() => preview.askFix(`\`assets/${asset.name}\``)}
                  >
                    <MessageSquarePlus size={12} />
                  </button>
                  <button type="button" className="iconbtn" title="Delete from the folder — versions keep their copy" onClick={() => void remove(asset)}>
                    <Trash2 size={12} />
                  </button>
                </div>
              </div>
              <span className="prototype-assets-name" title={`assets/${asset.name}`}>
                {asset.name}
              </span>
              <span className="prototype-assets-size">
                {asset.width !== null && asset.height !== null ? `${asset.width}×${asset.height}` : sizeLabel(asset.bytes)}
              </span>
            </li>
          ))}
          <li className="prototype-assets-item">
            <button type="button" className="prototype-assets-add" title="Add pictures to the chat's folder" onClick={() => void add()}>
              <Plus size={16} />
              <span>Add</span>
            </button>
          </li>
        </ul>
      )}
    </section>
  );
}
