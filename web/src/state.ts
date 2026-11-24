import { useEffect, useState } from "react";
import { emptyScope, scopeSchema } from "./api";
import type { Scope } from "./api";
import { isTimeWindow } from "./time-window";
import type { TimeWindow } from "./time-window";
export type Destination =
  | "watch"
  | "investigate"
  | "library"
  | "sources"
  | "workbench";
export type Route = {
  view: Destination;
  investigation: string | null;
  evidence: string | null;
  revision: string | null;
  messageKey: string | null;
  invalidScope: boolean;
  scope: Scope;
  window: TimeWindow | null;
  brief: string | null;
};
// Defaults stay out of the address bar. Legacy scope JSON remains readable.
export function parseRoute(search: string): Route {
  const p = new URLSearchParams(search);
  let scope = { ...emptyScope }, invalidScope = false;
  try {
    const legacy = p.has("scope") ? JSON.parse(p.get("scope")!) : null;
    const input = legacy ?? { ...emptyScope,
      source_ids: p.getAll("source"), include_unlocated: p.get("located") !== "1",
      query: p.get("q") ?? "", time_basis: p.get("time") === "received" ? "acquisition_time" : "publication_time",
      from: p.get("from"), to: p.get("to"), bbox: p.has("bbox") ? p.get("bbox")!.split(",").map(Number) : null,
      publication_id: p.get("snapshot"), content_view: p.get("feed") ?? "events", event_type: p.get("type"), related_key: p.get("related"),
    };
    const parsed = scopeSchema.safeParse(input);
    if (parsed.success) scope = parsed.data;
    else invalidScope = true;
    if (!legacy && ((p.has("time") && p.get("time") !== "received") || (p.has("located") && p.get("located") !== "1"))) invalidScope = true;
  } catch { invalidScope = true; }
  const view = p.get("view");
  const range = p.get("range");
  const window = isTimeWindow(range) ? range : null;
  if ((range && !window) || (window && (scope.from || scope.to))) invalidScope = true;
  return {
    view: view === "sources" ? "sources" : ["processing", "workbench"].includes(view ?? "") ? "workbench" : "investigate",
    investigation: p.get("investigation"), evidence: p.get("m") ? null : p.get("evidence"),
    revision: p.get("m") ? null : p.get("revision"), messageKey: p.get("m"), invalidScope, brief: p.get("brief"), scope, window,
  };
}
export function readRoute(): Route { return parseRoute(location.search); }
export function routeUrl(route: Route): string {
  const p = new URLSearchParams(), scope = route.scope;
  if (route.view === "sources") p.set("view", "sources");
  if (route.view === "workbench") p.set("view", "processing");
  if (route.messageKey) p.set("m", route.messageKey);
  else if (route.evidence && route.revision) { p.set("evidence", route.evidence); p.set("revision", route.revision); }
  for (const source of scope.source_ids) p.append("source", source);
  if (!scope.include_unlocated) p.set("located", "1");
  if (scope.query) p.set("q", scope.query);
  if (scope.time_basis === "acquisition_time") p.set("time", "received");
  if (route.window) p.set("range", route.window);
  else {
    if (scope.from) p.set("from", scope.from);
    if (scope.to) p.set("to", scope.to);
  }
  if (scope.bbox) p.set("bbox", scope.bbox.join(","));
  if (scope.publication_id) p.set("snapshot", scope.publication_id);
  // Event reports are the default view; other views are written to the URL.
  if (scope.content_view !== "events") p.set("feed", scope.content_view);
  if (scope.event_type) p.set("type", scope.event_type);
  if (scope.related_key) p.set("related", scope.related_key);
  const query = p.toString();
  return query ? `/?${query}` : "/";
}
export function nextRoute(old: Route, patch: Partial<Route>): Route {
  const next = { ...old, ...patch };
  if (patch.scope) next.invalidScope = false;
  if (patch.window) next.scope = { ...next.scope, from: null, to: null };
  if (patch.scope && patch.window === undefined && (patch.scope.from || patch.scope.to)) next.window = null;
  if ((patch.evidence !== undefined || patch.revision !== undefined) &&
      (next.evidence !== old.evidence || next.revision !== old.revision) && patch.messageKey === undefined)
    next.messageKey = null;
  if (patch.evidence === null) next.messageKey = null;
  return next;
}
export function useRoute() {
  const [route, setRoute] = useState(readRoute);
  useEffect(() => {
    const onPop = () => setRoute(readRoute());
    addEventListener("popstate", onPop);
    return () => removeEventListener("popstate", onPop);
  }, []);
  const navigate = (patch: Partial<Route>, replace = false) => {
    setRoute(old => {
      const next = nextRoute(old, patch);
      const url = routeUrl(next);
      // Keep the explicitly requested developer performance instrument working.
      const benchmark = new URLSearchParams(location.search).get("benchmark");
      const destination = benchmark ? `${url}${url.includes("?") ? "&" : "?"}benchmark=${encodeURIComponent(benchmark)}` : url;
      history[replace ? "replaceState" : "pushState"]({}, "", destination);
      return next;
    });
  };
  return [route, navigate] as const;
}
export function useDraft<T>(key: string, initial: T) {
  const [value, setValue] = useState<T>(() => {
    try {
      const v = localStorage.getItem(key);
      return v ? (JSON.parse(v) as T) : initial;
    } catch {
      return initial;
    }
  });
  const [storageError, setStorageError] = useState(false);
  const [recovered] = useState(() => {
    try {
      return localStorage.getItem(key) !== null;
    } catch {
      return false;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(key, JSON.stringify(value));
      setStorageError(false);
    } catch {
      setStorageError(true);
    }
  }, [key, value]);
  const clear = () => {
    try {
      localStorage.removeItem(key);
    } catch {
      setStorageError(true);
    }
  };
  return { value, setValue, recovered, storageError, clear };
}
