import { useEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { ImagePlusIcon, RefreshIcon, XIcon } from "../../brand/Icons";
import { useT } from "../../i18n";
import { MAX_IMAGES, type QueuedImage } from "./imageQueue";
import { Lightbox } from "./Lightbox";

/** Thumbnails above the textarea. */
export function ImageStrip({ items, onRemove, onRetry }: { items: QueuedImage[]; onRemove: (key: string) => void; onRetry: (item: QueuedImage) => void }) {
  const t = useT();
  const [open, setOpen] = useState<number | null>(null);
  return (
    <>
      {items.length > 0 && (
        <ul className="dock-images">
          {items.map((item, i) => (
            <li key={item.key} className={item.status}>
              <button type="button" className="dock-thumb" onClick={() => setOpen(i)} title={item.error ?? item.file.name} aria-label={t("image.open", { name: item.file.name })}>
                <img src={item.url} alt={item.file.name} />
              </button>
              {item.status === "uploading" && <span className="dock-thumb-state spinner" role="status" aria-label={t("composer.uploading")} />}
              {item.status === "error" && (
                <>
                  <span className="dock-thumb-dot" role="img" aria-label={t("composer.uploadFailed")} />
                  <button type="button" className="dock-thumb-state retry" title={`${t("composer.retryImage")} — ${item.error}`} aria-label={t("composer.retryImage")} onClick={() => onRetry(item)}>
                    <RefreshIcon size={13} />
                  </button>
                </>
              )}
              <button type="button" className="dock-thumb-remove" aria-label={t("composer.removeChip", { name: item.file.name })} onClick={() => onRemove(item.key)}>
                <XIcon size={10} stroke={2.4} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <Lightbox images={items.map((i) => ({ url: i.url, filename: i.file.name }))} index={open} onIndex={setOpen} />
    </>
  );
}

const hasFiles = (e: DragEvent) => !!e.dataTransfer?.types.includes("Files");

/**
 * Accepts files dropped anywhere on the chat (or new-chat) area around `anchor`, and covers
 * that area with a hint while they hover.
 */
export function DropZone({ anchor, disabled, onFiles }: { anchor: RefObject<HTMLElement | null>; disabled?: boolean; onFiles: (files: File[]) => void }) {
  const t = useT();
  const [target, setTarget] = useState<HTMLElement | null>(null);
  const [over, setOver] = useState(false);
  const files = useRef(onFiles);
  files.current = onFiles;

  useEffect(() => setTarget(anchor.current?.closest<HTMLElement>(".chat-main, .welcome") ?? null), [anchor]);

  useEffect(() => {
    if (!target || disabled) return;
    // `dragleave` also fires when moving between children; count to know when it really left.
    let depth = 0;
    const enter = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth += 1;
      setOver(true);
    };
    const move = (e: DragEvent) => hasFiles(e) && e.preventDefault();
    const leave = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      depth -= 1;
      if (depth <= 0) setOver(false);
    };
    const drop = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth = 0;
      setOver(false);
      files.current([...(e.dataTransfer?.files ?? [])]);
    };
    target.addEventListener("dragenter", enter);
    target.addEventListener("dragover", move);
    target.addEventListener("dragleave", leave);
    target.addEventListener("drop", drop);
    return () => {
      setOver(false);
      target.removeEventListener("dragenter", enter);
      target.removeEventListener("dragover", move);
      target.removeEventListener("dragleave", leave);
      target.removeEventListener("drop", drop);
    };
  }, [target, disabled]);

  if (!target || !over) return null;
  return createPortal(
    <div className="drop-overlay">
      <div>
        <ImagePlusIcon size={22} />
        <span>{t("composer.dropImages", { max: MAX_IMAGES })}</span>
      </div>
    </div>,
    target,
  );
}
