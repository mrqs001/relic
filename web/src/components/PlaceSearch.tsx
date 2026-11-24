import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { ArrowUpRight, Globe2, MapPin, Search, X } from "lucide-react";

const placeSchema = z.object({
  id: z.string(), name: z.string(), kind: z.enum(["country", "city"]),
  country: z.string().optional(), aliases: z.array(z.string()),
  longitude: z.number().min(-180).max(180), latitude: z.number().min(-90).max(90), zoom: z.number().min(0).max(20),
});
type Place = z.infer<typeof placeSchema>;
const normalize = (value: string) => value.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLocaleLowerCase();
export default function PlaceSearch({ onPlace, onReports }: {
  onPlace: (place: Place) => void; onReports: (query: string) => void;
}) {
  const [text, setText] = useState(""), [open, setOpen] = useState(false), [visible, setVisible] = useState(false), [active, setActive] = useState(0);
  const launcher = useRef<HTMLButtonElement>(null);
  const input = useRef<HTMLInputElement>(null), host = useRef<HTMLDivElement>(null);
  const places = useQuery({
    queryKey: ["map-places"], staleTime: Infinity, enabled: visible,
    queryFn: async ({ signal }) => {
      const response = await fetch("/map-places.json", { signal });
      if (!response.ok) throw new Error("Place search unavailable");
      return z.array(placeSchema).max(1000).parse(await response.json());
    },
  });
  const matches = useMemo(() => {
    const term = normalize(text.trim());
    if (!term) return [];
    return (places.data || []).map(place => {
      const names = [place.name, ...place.aliases].map(normalize);
      const score = names.some(n => n === term) ? 0 : names.some(n => n.startsWith(term)) ? 1 : names.some(n => n.includes(term)) ? 2 : 3;
      return { place, score };
    }).filter(p => p.score < 3).sort((a, b) => a.score - b.score || a.place.name.localeCompare(b.place.name)).slice(0, 6).map(p => p.place);
  }, [text, places.data]);
  useEffect(() => {
    const keyboard = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") { e.preventDefault(); setVisible(true); setOpen(true); input.current?.focus(); }
    };
    const outside = (e: PointerEvent) => { if (!host.current?.contains(e.target as Node)) { setOpen(false); setVisible(false); } };
    document.addEventListener("keydown", keyboard); document.addEventListener("pointerdown", outside);
    return () => { document.removeEventListener("keydown", keyboard); document.removeEventListener("pointerdown", outside); };
  }, []);
  useEffect(() => { if (visible) input.current?.focus(); }, [visible]);
  const choose = (index: number) => {
    const place = matches[index];
    if (place) { onPlace(place); setText(place.name); }
    else onReports(text.trim());
    setOpen(false); setVisible(false); launcher.current?.focus();
  };
  const expanded = open && !!text.trim();
  return <div className={`place-search compact-search ${visible ? "is-open" : ""}`} ref={host}>
    <button className="search-launcher" ref={launcher} aria-label="Search" title="Search · Ctrl / ⌘ K" aria-expanded={visible} onClick={() => { setVisible(!visible); setOpen(true); }}><Search size={18}/></button>
    {visible && <div className="place-search-field">
      <Search size={17} strokeWidth={1.6} />
      <input ref={input} role="combobox" aria-label="Search places or reports" aria-autocomplete="list" aria-expanded={expanded} aria-controls="place-results" aria-activedescendant={expanded ? `place-option-${Math.min(active, matches.length)}` : undefined}
        value={text} placeholder="Search places or reports" autoComplete="off"
        onFocus={() => setOpen(true)} onChange={e => { setText(e.target.value); setActive(0); setOpen(true); }}
        onKeyDown={e => {
          if (e.key === "Escape") { e.stopPropagation(); setOpen(false); setVisible(false); launcher.current?.focus(); }
          if (e.key === "ArrowDown") { e.preventDefault(); setOpen(true); setActive(i => Math.min(i + 1, matches.length)); }
          if (e.key === "ArrowUp") { e.preventDefault(); setActive(i => Math.max(i - 1, 0)); }
          if (e.key === "Enter" && text.trim()) { e.preventDefault(); choose(Math.min(active, matches.length)); }
        }} />
      {text ? <button className="icon-button" aria-label="Clear place search" onClick={() => { setText(""); input.current?.focus(); }}><X size={14}/></button> : <kbd>⌘ K</kbd>}
    </div>}
    {visible && expanded && <div className="place-results" id="place-results" role="listbox" aria-label="Search results">
      {matches.map((p, i) => <button key={p.id} id={`place-option-${i}`} role="option" aria-selected={active === i} onMouseEnter={() => setActive(i)} onClick={() => choose(i)}>
        {p.kind === "country" ? <Globe2 size={17}/> : <MapPin size={17}/>}
        <span><strong>{p.name}</strong><small>{p.kind === "country" ? "Country / territory" : p.country || "City"}</small></span><ArrowUpRight size={14}/>
      </button>)}
      <button id={`place-option-${matches.length}`} role="option" aria-selected={active === matches.length} onMouseEnter={() => setActive(matches.length)} onClick={() => choose(matches.length)}><Search size={16}/><span><strong>Search reports for “{text.trim()}”</strong><small>Filter retained evidence</small></span><ArrowUpRight size={14}/></button>
      {places.isError && <p className="search-disclosure">Place index unavailable. Report search still works.</p>}
      <p className="search-disclosure">Places move the camera. Reports filter the evidence.</p>
    </div>}
  </div>;
}
