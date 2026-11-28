import { useCallback, useEffect, useRef, useState } from "react";
import { attachments as attachmentsApi, errorText } from "../../api";

export const MAX_IMAGES = 6;
export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/webp", "image/gif"];
const MAX_EDGE = 2048;
/** Anthropic limits an image to 5 MB *after* Base64, which is about 3.9 MB of file. */
const MAX_BYTES = 3_900_000;

export interface QueuedImage {
  key: string;
  file: File;
  /** Object URL of the original, for the thumbnail and the preview before sending. */
  url: string;
  status: "uploading" | "done" | "error";
  /** The server's attachment id once uploaded. */
  id?: string;
  width?: number;
  height?: number;
  error?: string;
}

const encode = (canvas: HTMLCanvasElement, type: string, quality: number) =>
  new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, type, quality));

/**
 * Shrinks an image that is too large for the providers (long edge or file size), keeping its
 * aspect ratio. GIFs go through untouched so animations survive.
 */
async function prepare(file: File): Promise<File> {
  if (file.type === "image/gif") return file;
  const bitmap = await createImageBitmap(file);
  try {
    const edge = Math.max(bitmap.width, bitmap.height);
    if (edge <= MAX_EDGE && file.size <= MAX_BYTES) return file;
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");
    if (!ctx) return file;
    let scale = Math.min(1, MAX_EDGE / edge);
    let quality = 0.9;
    for (let attempt = 0; attempt < 6; attempt++) {
      canvas.width = Math.max(1, Math.round(bitmap.width * scale));
      canvas.height = Math.max(1, Math.round(bitmap.height * scale));
      ctx.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
      let blob = await encode(canvas, "image/webp", quality);
      // Browsers without a WebP encoder answer with PNG; use JPEG on white there.
      if (blob?.type !== "image/webp") {
        ctx.globalCompositeOperation = "destination-over";
        ctx.fillStyle = "#fff";
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.globalCompositeOperation = "source-over";
        blob = await encode(canvas, "image/jpeg", quality);
      }
      if (blob && blob.size <= MAX_BYTES) {
        const ext = blob.type === "image/webp" ? "webp" : "jpg";
        return new File([blob], `${file.name.replace(/\.[^.]*$/, "") || "image"}.${ext}`, { type: blob.type });
      }
      scale *= 0.8;
      quality = Math.max(0.6, quality - 0.1);
    }
    return file;
  } finally {
    bitmap.close();
  }
}

/** The images waiting to be sent with the next message, each uploaded as soon as it is added. */
export function useImageQueue(onRejected: (why: "limit" | "type") => void) {
  const [items, setItems] = useState<QueuedImage[]>([]);
  // Mirrors the state so that several pastes in one tick still respect the limit.
  const list = useRef<QueuedImage[]>([]);
  const update = useCallback((f: (all: QueuedImage[]) => QueuedImage[]) => {
    list.current = f(list.current);
    setItems(list.current);
  }, []);
  const patch = useCallback((key: string, change: Partial<QueuedImage>) => update((all) => all.map((i) => (i.key === key ? { ...i, ...change } : i))), [update]);

  const upload = useCallback(
    async (key: string, file: File) => {
      patch(key, { status: "uploading", error: undefined });
      try {
        // An image the browser cannot decode is uploaded as it is; the server decides.
        const ready = await prepare(file).catch(() => file);
        const a = await attachmentsApi.upload(ready);
        patch(key, { status: "done", id: a.id, width: a.width ?? undefined, height: a.height ?? undefined });
      } catch (e) {
        patch(key, { status: "error", error: errorText(e) });
      }
    },
    [patch],
  );

  const add = useCallback(
    (files: Iterable<File>) => {
      const all = [...files];
      const images = all.filter((f) => IMAGE_TYPES.includes(f.type));
      if (images.length < all.length) onRejected("type");
      const room = MAX_IMAGES - list.current.length;
      if (images.length > room) onRejected("limit");
      const added = images.slice(0, Math.max(0, room)).map<QueuedImage>((file) => ({
        key: `${Date.now()}-${Math.random()}`,
        file,
        url: URL.createObjectURL(file),
        status: "uploading",
      }));
      if (!added.length) return;
      update((cur) => [...cur, ...added]);
      for (const item of added) upload(item.key, item.file);
    },
    [onRejected, update, upload],
  );

  const remove = useCallback(
    (keys: string[]) => {
      for (const i of list.current) if (keys.includes(i.key)) URL.revokeObjectURL(i.url);
      update((all) => all.filter((i) => !keys.includes(i.key)));
    },
    [update],
  );

  useEffect(() => () => list.current.forEach((i) => URL.revokeObjectURL(i.url)), []);

  return { items, add, remove, retry: (item: QueuedImage) => upload(item.key, item.file) };
}
