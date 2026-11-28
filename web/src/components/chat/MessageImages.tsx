import { useState } from "react";
import type { ContentPart, ImageAttachment } from "../../api/types";
import { attachmentUrl } from "../../api";
import { useT } from "../../i18n";
import { Lightbox, type LightboxImage } from "./Lightbox";

/** The stored images among a message's blocks. */
export const imagesOf = (content: ContentPart[]): ImageAttachment[] =>
  content.flatMap((p) => (p.kind === "image" && p.source.type === "attachment" ? [p.source] : []));

/** The image grid of one message: 1 natural, 2 side by side, 3–6 in rows of three. */
export function MessageImages({ images }: { images: ImageAttachment[] }) {
  const t = useT();
  const [open, setOpen] = useState<number | null>(null);
  const shown: LightboxImage[] = images.map((i) => ({ url: attachmentUrl(i.attachment_id), filename: i.filename, width: i.width, height: i.height }));
  return (
    <>
      <div className={`msg-images count-${Math.min(images.length, 3)}`}>
        {shown.map((image, i) => (
          <button key={i} type="button" onClick={() => setOpen(i)} aria-label={t("image.open", { name: image.filename ?? `${i + 1}` })}>
            <img src={image.url} alt={image.filename ?? t("image.alt")} width={image.width} height={image.height} />
          </button>
        ))}
      </div>
      <Lightbox images={shown} index={open} onIndex={setOpen} />
    </>
  );
}
