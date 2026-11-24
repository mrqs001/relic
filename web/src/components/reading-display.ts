import type { EvidencePreview as Evidence } from "../api";

export const claimStatusLabel = { reported: "Reported", denied: "Denied", planned: "Planned", historical: "Historical", unclear: "Unclear" } as const;

export function shortRevision(revision: string) {
  return revision.length > 24 ? `${revision.slice(0, revision.startsWith("sha256:") ? 19 : 16)}…` : revision;
}

export function readingPreview(evidence: Pick<Evidence, "text" | "interpretation" | "upstream_state" | "media">) {
  const interpretation = evidence.interpretation;
  return cleanReadingText((readingUsesSummary(evidence) ? interpretation?.headline : interpretation?.translation?.trim()) || evidence.text) || (evidence.upstream_state === "removed" ? "Removed message" : evidence.media?.length ? "Media-only message" : "No retained text");
}

export function evidenceTimestamp(evidence: Pick<Evidence, "published_at" | "acquired_at">) {
  return { value: evidence.published_at || evidence.acquired_at, label: evidence.published_at ? "Published" : "Received" };
}

export function siteLabel(evidence: Pick<Evidence, "map_locations" | "map_areas" | "interpretation">) {
  const sites = [...evidence.map_locations, ...evidence.map_areas];
  if (sites.length > 2) return `${sites.length} places`;
  return sites.map(site => placeLabel(site, evidence.interpretation)).join(" · ");
}

// Only the confirmed recurring sponsor footer. Never mutate evidence or Original,
// or remove ordinary reporting that merely mentions the sponsor.
export function cleanReadingText(text: string) {
  return text.replace(/^\s*(?:💧\s*)?Rainbet\.com the #1 Non-KYC Crypto Casino & Sportsbook @rainbetcom(?: \+18)?\s*$/gmi, "")
    .replace(/^\s*@wfwitness\s*$/gmi, "").replace(/\n{3,}/g, "\n\n").trim();
}
const countries = new Intl.DisplayNames(["en"], { type: "region" });
export function countryPlace(label: string | null | undefined, code?: string | null) {
  const name = label?.replaceAll("_", " ") || "Source location";
  const country = code && /^[A-Z]{2}$/.test(code) ? countries.of(code) : undefined;
  return country && name.toLowerCase() !== country.toLowerCase() ? `${country}, ${name}` : name;
}
const record = (value: unknown): Record<string, unknown> => value && typeof value === "object" ? value as Record<string, unknown> : {};
type Site = {id: string; label: string | null; country_code?: string | null; level?: string};
/** The site's country code: its own, else the reading's resolution of it. */
function siteCountry(site: Site, reading?: Evidence["interpretation"]) {
  let country = site.country_code;
  if (!country && reading) {
    const resolutions = [...reading.locations, ...reading.events.flatMap(event => event.locations)];
    for (const value of resolutions) {
      const resolution = record(value);
      const selected = resolution.selected_candidate_id;
      if (typeof selected !== "string" || !Array.isArray(resolution.candidates)) continue;
      const candidate = resolution.candidates.map(record).find(c => c.id === selected);
      if (candidate && (selected === site.id || (site.id === "single" && candidate.name === site.label))) {
        if (typeof candidate.country_code === "string") country = candidate.country_code;
        break;
      }
    }
  }
  return country && /^[A-Z]{2}$/.test(country) ? country : null;
}
export function placeLabel(site: Site, reading?: Evidence["interpretation"]) {
  const country = siteCountry(site, reading);
  if (site.level === "ADM0" && country) return countries.of(country) || site.label || country;
  return countryPlace(site.label, country);
}
// Most specific first: a reported point, a facility or place, a district,
// a province or sea, then a country.
const specificity: Record<string, number> = { FACILITY: 5, PLACE: 5, ADM2: 4, ADM1: 3, MARINE: 3, ADM0: 0 };
/** A feed row's place: the most specific site in bold, its country beside it,
 *  and how many other places the message names. "Dar'a" + "Syria", never
 *  "Syria · Syria, Dar'a". */
export function feedPlace(evidence: Pick<Evidence, "map_locations" | "map_areas" | "interpretation">) {
  const reading = evidence.interpretation;
  const sites = [
    ...evidence.map_locations.map(site => ({ site, rank: 6 })),
    ...evidence.map_areas.map(site => ({ site, rank: specificity[site.level] ?? 2 })),
  ].map(({ site, rank }) => {
    const code = siteCountry(site, reading);
    const country = code ? countries.of(code) ?? null : null;
    const name = rank === 0 ? country ?? site.label?.replaceAll("_", " ") ?? null : site.label?.replaceAll("_", " ") ?? null;
    return { name, country: rank === 0 ? null : country, rank };
  }).filter(site => site.name).sort((a, b) => b.rank - a.rank);
  if (!sites.length) return null;
  const [top, ...rest] = sites;
  const covered = new Set([top.name, top.country].filter(Boolean).map(name => name!.toLowerCase()));
  const others = new Set(rest.map(site => site.name!.toLowerCase()).filter(name => !covered.has(name)));
  const country = top.country && top.country.toLowerCase() !== top.name!.toLowerCase() ? top.country : null;
  return { place: top.name!, country, more: others.size };
}
export function relativeAge(value: string, now: number) {
  const delta = now - Date.parse(value);
  if (!Number.isFinite(delta)) return "";
  if (delta < -60000) return "in the future";
  if (delta < 60000) return "just now";
  const minutes = Math.floor(delta / 60000);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h ago` : `${Math.floor(hours / 24)}d ago`;
}

export function untranslatedCopy(evidence: Pick<Evidence, "text" | "interpretation">) {
  const reading = evidence.interpretation;
  return !!reading && reading.language !== "en" && reading.translation?.trim() === evidence.text.trim()
    && Array.from(evidence.text).filter(c => c.codePointAt(0)! > 0x024f && /\p{Letter}/u.test(c)).length >= 3;
}

// Same deliberately narrow prose-block guard as the Rust processor. This is
// not language detection and never removes a presumed duplicate from evidence.
export function mixedScriptParagraphs(text: string) {
  let latin = false, nonLatin = false;
  for (const line of text.split(/\r?\n/)) {
    const letters = Array.from(line).filter(c => /\p{Alphabetic}/u.test(c));
    if (letters.length < 40) continue;
    latin ||= letters.filter(c => /[A-Za-z]/.test(c)).length * 5 >= letters.length * 4;
    nonLatin ||= letters.filter(c => c.codePointAt(0)! > 0x024f).length * 5 >= letters.length * 4;
  }
  return latin && nonLatin;
}
export function readingUsesSummary(evidence: Pick<Evidence, "text" | "interpretation">) {
  const reading = evidence.interpretation;
  return untranslatedCopy(evidence) || !!reading && reading.language === "en"
    && !reading.translation && mixedScriptParagraphs(evidence.text);
}
