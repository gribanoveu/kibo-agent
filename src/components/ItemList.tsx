import { useState } from "react";
import { ChevronRight, Pencil, Plus, Trash2 } from "lucide-react";
import type { PanelItem } from "../types";
import "./ItemList.css";

type ItemProps = {
  item: PanelItem;
  onToggle?: (id: string, enabled: boolean) => void;
  onToggleRow?: (id: string, row: string, enabled: boolean) => void;
  onOpen?: (id: string) => void;
  onEdit?: (id: string) => void;
  onRemove?: (id: string) => void;
};

function Item({ item, onToggle, onToggleRow, onOpen, onEdit, onRemove }: ItemProps) {
  const [open, setOpen] = useState(false);
  // Removing asks once more, in the row itself: it rewrites the user's file.
  const [confirming, setConfirming] = useState(false);
  const enabled = item.enabled ?? false;

  return (
    <div className={`item${open ? " open" : ""}${item.dim ? " dim" : ""}`}>
      <div
        className="item-head"
        onClick={() => {
          if (!open) onOpen?.(item.id);
          setOpen((v) => !v);
        }}
      >
        <span className={`item-ico ${item.kind}`}>{item.badge}</span>
        <div className="item-body">
          <div className="item-title">
            {item.title}
            {item.status && (
              <span className={`badge-pill ${item.status.tone}`}>{item.status.label}</span>
            )}
          </div>
          <div className="item-desc">{item.desc}</div>
          {(item.meta || item.tags) && (
            <div className="item-meta">
              {item.meta}
              {item.tags?.map((tag) => (
                <span className="skill-tag" key={tag}>
                  {tag}
                </span>
              ))}
            </div>
          )}
        </div>
        <div className="item-actions">
          {item.enabled !== undefined && onToggle && (
            <button
              type="button"
              className={`toggle${enabled ? " on" : ""}`}
              aria-pressed={enabled}
              aria-label={enabled ? "Enabled" : "Disabled"}
              onClick={(e) => {
                e.stopPropagation();
                onToggle(item.id, !enabled);
              }}
            />
          )}
          <ChevronRight className="item-chev" size={12} />
        </div>
      </div>
      {open && (
        <div className="item-detail">
          {item.rows?.map((row) => (
            <div className={`tool-row${row.enabled === false ? " off" : ""}`} key={row.id ?? row.name}>
              <span className="tname">{row.name}</span>
              <span className="tdesc">{row.desc}</span>
              {row.enabled !== undefined && onToggleRow && (
                <button
                  type="button"
                  className={`toggle${row.enabled ? " on" : ""}`}
                  aria-pressed={row.enabled}
                  aria-label={`${row.id ?? row.name}: ${row.enabled ? "offered to the model" : "hidden from the model"}`}
                  onClick={() => onToggleRow(item.id, row.id ?? row.name, !row.enabled)}
                />
              )}
            </div>
          ))}
          {item.note &&
            (item.kind === "rule" ? (
              <div className="rule-block">{item.note}</div>
            ) : (
              <div className="empty item-note">{item.note}</div>
            ))}
          {item.source && <div className="rule-source">{item.source}</div>}
          {(onEdit || onRemove) && (
            <div className="item-row-actions">
              {confirming ? (
                <>
                  <span className="item-confirm">Remove {item.title} from the file?</span>
                  <button type="button" className="link-btn danger" onClick={() => onRemove?.(item.id)}>
                    Remove
                  </button>
                  <button type="button" className="link-btn" onClick={() => setConfirming(false)}>
                    Keep
                  </button>
                </>
              ) : (
                <>
                  {onEdit && (
                    <button type="button" className="link-btn" onClick={() => onEdit(item.id)}>
                      <Pencil size={11} />
                      Edit
                    </button>
                  )}
                  {onRemove && (
                    <button type="button" className="link-btn danger" onClick={() => setConfirming(true)}>
                      <Trash2 size={11} />
                      Remove
                    </button>
                  )}
                </>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

type Props = {
  label: string;
  count: string;
  items: PanelItem[];
  emptyLabel: string;
  addLabel?: string;
  onAdd?: () => void;
  /** Without it the items have no switch: the list cannot change what it shows. */
  onToggle?: (id: string, enabled: boolean) => void;
  /** A switch on each row of an item's detail that has `enabled`. */
  onToggleRow?: (id: string, row: string, enabled: boolean) => void;
  /** Called when a row is expanded, for details that are fetched rather than held. */
  onOpen?: (id: string) => void;
  /** Per-row actions in the expanded row. Remove asks to confirm first. */
  onEdit?: (id: string) => void;
  onRemove?: (id: string) => void;
  /** A link in the heading to the whole file the list is read from. */
  onEditFile?: () => void;
};

export function ItemList({
  label,
  count,
  items,
  emptyLabel,
  addLabel,
  onAdd,
  onToggle,
  onToggleRow,
  onOpen,
  onEdit,
  onRemove,
  onEditFile,
}: Props) {
  return (
    <div className="panel-section">
      <div className="section-label">
        <span>{label}</span>
        <span className="section-tail">
          {onEditFile && (
            <button type="button" className="link-btn" onClick={onEditFile}>
              Edit JSON
            </button>
          )}
          <span className="count">{count}</span>
        </span>
      </div>
      {items.length === 0 && <div className="empty">{emptyLabel}</div>}
      {items.map((item) => (
        <Item
          key={item.id}
          item={item}
          onToggle={onToggle}
          onToggleRow={onToggleRow}
          onOpen={onOpen}
          onEdit={onEdit}
          onRemove={onRemove}
        />
      ))}
      {addLabel && (
        <button className="add-btn" type="button" onClick={onAdd}>
          <Plus size={12} />
          {addLabel}
        </button>
      )}
    </div>
  );
}
