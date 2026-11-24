export type MediaIdentity = { id: string; revision: string; index: number };

// Telegram may reconcile an actual edit while refreshing the attachment.
// Validate the returned route against that explicit revision before displaying it.
export function resolvedMediaReceipt<T extends { url: string; revision?: string }>(receipt: T, requested: MediaIdentity) {
  const revision = receipt.revision ?? requested.revision;
  if (revision !== requested.revision && (!requested.id.startsWith("telegram:") || !/^sha256:[a-f0-9]{64}$/.test(revision)))
    throw new Error("Media returned an invalid message revision.");
  return { ...receipt, revision, url: controlledMediaUrl(receipt.url, { ...requested, revision }) };
}

// A receipt can point only at the same exact revision's controlled cache route.
// Never pass retained provider URLs, redirects, blobs, or data URLs to the DOM.
export function controlledMediaUrl(value: string, identity: MediaIdentity) {
  if (!value.startsWith("/") || value.startsWith("//"))
    throw new Error("Media did not return a local retained file.");
  const url = new URL(value, "http://relic.local");
  if (
    url.origin !== "http://relic.local" || url.pathname !== "/api/v1/media" || url.hash ||
    url.searchParams.size !== 3 || url.searchParams.get("id") !== identity.id ||
    url.searchParams.get("revision") !== identity.revision ||
    url.searchParams.get("index") !== String(identity.index)
  ) throw new Error("Media did not match this exact retained revision.");
  return `${url.pathname}${url.search}`;
}

export function mediaLabel(reference: unknown) {
  const item = reference && typeof reference === "object" ? reference as Record<string, unknown> : {};
  const mime = typeof item.mime_type === "string" ? item.mime_type.toLowerCase() : "";
  const kind = typeof item.kind === "string" ? item.kind.toLowerCase() : "";
  if (kind === "photo" || mime.startsWith("image/")) return "Image";
  if (["video", "animated_gif"].includes(kind) || mime.startsWith("video/")) return "Video";
  if (["audio", "voice"].includes(kind) || mime.startsWith("audio/")) return "Audio";
  return kind === "document" ? "Attachment" : "Media";
}

export function mediaPresentation(mediaType: string): "image" | "video" | "audio" | "download" {
  const mime = mediaType.split(";")[0].trim().toLowerCase();
  if (["image/jpeg", "image/png", "image/webp", "image/gif", "image/avif"].includes(mime)) return "image";
  if (["video/mp4", "video/webm", "video/ogg"].includes(mime)) return "video";
  if (["audio/mpeg", "audio/mp4", "audio/ogg", "audio/wav", "audio/webm", "audio/flac"].includes(mime)) return "audio";
  return "download";
}
