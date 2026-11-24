import { z } from "zod";
import { ApiError, mediaReceiptSchema } from "../api";
import { resolvedMediaReceipt } from "./evidence-media";
import type { MediaIdentity } from "./evidence-media";

const progressSchema = z.object({
  kind: z.literal("progress"),
  phase: z.enum(["connecting", "downloading", "verifying"]),
  downloaded: z.number().int().nonnegative().max(512 * 1024 * 1024),
  total: z.number().int().positive().max(512 * 1024 * 1024).nullable(),
}).refine(v => v.total === null || v.downloaded <= v.total);
const eventSchema = z.discriminatedUnion("kind", [
  progressSchema,
  z.object({ kind: z.literal("ready"), receipt: mediaReceiptSchema }),
  z.object({ kind: z.literal("error"), code: z.string(), message: z.string() }),
]);
export type MediaProgress = z.infer<typeof progressSchema>;
export type MediaReceipt = z.infer<typeof mediaReceiptSchema> & { revision: string };

// No background mutation or retry: the reader owns this response and its abort
// signal. Closing it drops the server's upstream future and private staging.
export async function downloadMedia(identity: MediaIdentity, signal: AbortSignal, progress: (value: MediaProgress) => void): Promise<MediaReceipt> {
  const response = await fetch("/api/v1/media/download", {
    method: "POST", credentials: "same-origin", signal,
    headers: { "Content-Type": "application/json" }, body: JSON.stringify(identity),
  });
  if (!response.ok) {
    const body = await response.json().catch(() => null);
    throw new ApiError(body?.error?.message ?? "Media could not be loaded.", body?.error?.code ?? "request_failed", response.status);
  }
  if (!response.body || !response.headers.get("content-type")?.startsWith("application/x-ndjson")) {
    await response.body?.cancel();
    throw new Error("Media returned an invalid download response.");
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let pending = "";
  try {
    while (true) {
      signal.throwIfAborted();
      const { value, done } = await reader.read();
      signal.throwIfAborted();
      pending += decoder.decode(value, { stream: !done });
      let newline: number;
      while ((newline = pending.indexOf("\n")) >= 0) {
        if (newline > 16 * 1024) throw new Error("Media progress exceeded its response limit.");
        const line = pending.slice(0, newline);
        pending = pending.slice(newline + 1);
        const parsed = eventSchema.safeParse(JSON.parse(line));
        if (!parsed.success) throw new Error("Media returned invalid download progress.");
        const event = parsed.data;
        if (event.kind === "error") throw new ApiError(event.message, event.code, 200);
        if (event.kind === "ready") return resolvedMediaReceipt(event.receipt, identity);
        progress(event);
      }
      if (pending.length > 16 * 1024) throw new Error("Media progress exceeded its response limit.");
      if (done) throw new Error("The download ended before the media was ready. Try again.");
    }
  } finally {
    // Also cancel on invalid responses; releaseLock alone would keep fetching.
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export function mediaBytes(bytes: number) {
  const unit = bytes >= 1024 * 1024 ? 1024 * 1024 : 1024;
  return new Intl.NumberFormat("en-GB", { maximumFractionDigits: 1 }).format(bytes / unit) + (unit === 1024 ? " KB" : " MB");
}
