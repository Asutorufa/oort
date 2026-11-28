import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AnimatePresence, motion } from "motion/react";
import { ChevronLeftIcon, ChevronRightIcon, DownloadIcon, XIcon } from "../../brand/Icons";
import { useT } from "../../i18n";
import { fade } from "../../lib/motion";

export interface LightboxImage {
  url: string;
  filename?: string;
  width?: number;
  height?: number;
}

/** Full-screen preview of the images of one message. `index === null` is closed. */
export function Lightbox({ images, index, onIndex }: { images: LightboxImage[]; index: number | null; onIndex: (i: number | null) => void }) {
  return createPortal(
    <AnimatePresence>{index !== null && images[index] && <Viewer key="viewer" images={images} index={index} onIndex={onIndex} />}</AnimatePresence>,
    document.body,
  );
}

function Viewer({ images, index, onIndex }: { images: LightboxImage[]; index: number; onIndex: (i: number | null) => void }) {
  const t = useT();
  const root = useRef<HTMLDivElement>(null);
  const [natural, setNatural] = useState<Record<string, string>>({});
  const image = images[index];
  const many = images.length > 1;
  const step = (d: number) => onIndex((index + d + images.length) % images.length);

  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    root.current?.focus();
    return () => before?.focus();
  }, []);

  useEffect(() => {
    // Captured so that Escape closes only the preview, not what lies under it.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onIndex(null);
      else if (e.key === "ArrowLeft" && many) step(-1);
      else if (e.key === "ArrowRight" && many) step(1);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  const size = image.width && image.height ? `${image.width}×${image.height}` : natural[image.url];
  const name = image.filename ?? t("image.alt");

  return (
    <motion.div
      ref={root}
      className="lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={name}
      tabIndex={-1}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={fade}
      onClick={(e) => e.target === e.currentTarget && onIndex(null)}
    >
      <button type="button" className="lightbox-btn lightbox-close" aria-label={t("image.close")} title={t("image.close")} onClick={() => onIndex(null)}>
        <XIcon size={18} />
      </button>
      {many && (
        <button type="button" className="lightbox-btn lightbox-prev" aria-label={t("image.prev")} title={t("image.prev")} onClick={() => step(-1)}>
          <ChevronLeftIcon size={20} />
        </button>
      )}
      <img
        key={image.url}
        src={image.url}
        alt={name}
        onLoad={(e) => {
          const { naturalWidth: w, naturalHeight: h } = e.currentTarget;
          setNatural((n) => ({ ...n, [image.url]: `${w}×${h}` }));
        }}
      />
      {many && (
        <button type="button" className="lightbox-btn lightbox-next" aria-label={t("image.next")} title={t("image.next")} onClick={() => step(1)}>
          <ChevronRightIcon size={20} />
        </button>
      )}
      <div className="lightbox-bar">
        {many && (
          <span className="lightbox-count">
            {index + 1} / {images.length}
          </span>
        )}
        {image.filename && <span className="lightbox-name">{image.filename}</span>}
        {size && <span>{size}</span>}
        <a className="lightbox-btn" href={image.url} download={image.filename ?? "image"} aria-label={t("image.download")} title={t("image.download")}>
          <DownloadIcon size={15} />
        </a>
      </div>
    </motion.div>
  );
}
