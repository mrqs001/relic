import type { EvidencePreview } from "../api";

// Presentation of an explicit Telegram album, never semantic evidence merging.
// Only members loaded under this exact scope/publication may enter the row.
export function feedAlbums(messages: EvidencePreview[]) {
  const groups = new Map<string, EvidencePreview[]>();
  for (const message of messages) {
    const album = message.relationships?.grouped_id;
    const key = message.source.startsWith("telegram:") && typeof album === "string" && album
      ? JSON.stringify([message.source,album]) : JSON.stringify([message.id,message.revision]);
    const group=groups.get(key);
    if (group) group.push(message); else groups.set(key,[message]);
  }
  return [...groups.values()].map(members=>({
    message:members.find(message=>message.text.trim()) ?? members[0], members,
  }));
}

export type FeedRow = ReturnType<typeof feedAlbums>[number] & { reports: ReturnType<typeof feedAlbums> };
/** Reports of one linked event fold under the newest one, each keeping its own
 *  album. Presentation only: every report stays one click away, and the event
 *  group comes from reviewed links in this exact scope. */
export function feedRows(messages: EvidencePreview[]): FeedRow[] {
  const rows: FeedRow[] = [];
  const byEvent = new Map<string, number>();
  for (const album of feedAlbums(messages)) {
    const group = album.members.map(m => m.event_group?.id).find(Boolean);
    const at = group ? byEvent.get(group) : undefined;
    if (at !== undefined) { rows[at].reports.push(album); continue; }
    if (group) byEvent.set(group, rows.length);
    rows.push({ ...album, reports: [] });
  }
  return rows;
}
