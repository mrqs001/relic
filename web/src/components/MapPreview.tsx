import { useEffect, useMemo, useRef } from "react";
import type { MutableRefObject } from "react";
import { createPortal } from "react-dom";
import maplibregl from "maplibre-gl";
import { ArrowRight, X } from "lucide-react";
import type { EventType } from "../api";
import { eventColors, eventLabels } from "./EventLabel";
import { relativeAge } from "./reading-display";

export type PreviewContent = {
  source: string;
  time: string | null;
  place: { place: string; country: string | null } | null;
  event: { type: EventType; possible: boolean } | null;
  text: string;
};

/** The selected message as a card on the map, as Recall showed it: where,
 *  what, the source's words, and Open for the full message. The feed stays. */
export default function MapPreview({ map, ready, at, content, onOpen, onClose }: {
  map: MutableRefObject<maplibregl.Map | null>;
  ready: boolean;
  at: [number, number] | null;
  content: PreviewContent | null;
  onOpen: () => void;
  onClose: () => void;
}) {
  const host = useMemo(() => document.createElement("div"), []);
  const popup = useRef<maplibregl.Popup | null>(null);
  // The card node is inserted once and only moved when its coordinates change:
  // re-inserting it on every render replayed its entrance (a flicker on pan).
  const [lng, lat] = at ?? [NaN, NaN], open = !!content && Number.isFinite(lng) && Number.isFinite(lat);
  useEffect(() => {
    const m = map.current;
    if (!ready || !m || !open) { popup.current?.remove(); return; }
    if (!popup.current) popup.current = new maplibregl.Popup({ closeButton: false, closeOnClick: false, offset: 16, maxWidth: "330px", className: "relic-preview-popup", focusAfterOpen: false }).setDOMContent(host);
    popup.current.setLngLat([lng, lat]);
    if (!popup.current.isOpen()) popup.current.addTo(m);
  }, [map, ready, open, lng, lat, host]);
  useEffect(() => () => { popup.current?.remove(); }, []);
  if (!content || !at) return null;
  const color = content.event ? eventColors[content.event.type] : undefined;
  return createPortal(
    <article className="map-preview" aria-label="Selected message">
      <header>
        {content.event
          ? <span className={`map-preview-type ${content.event.possible ? "possible" : ""}`} style={{ "--event": color } as React.CSSProperties}><i/>{eventLabels[content.event.type]}{content.event.possible && <small>possible</small>}</span>
          : <span className="map-preview-type none">Message</span>}
        <button className="map-preview-close" aria-label="Close preview" onClick={onClose}><X size={14}/></button>
      </header>
      {content.place && <h2>{content.place.place}{content.place.country && <span>{content.place.country}</span>}</h2>}
      <p dir="auto">{content.text}</p>
      <footer>
        <span>{content.source}{content.time && <> · <time dateTime={content.time} title={new Date(content.time).toUTCString()}>{relativeAge(content.time, Date.now())}</time></>}</span>
        <button className="map-preview-open" onClick={onOpen}>Open<ArrowRight size={13}/></button>
      </footer>
    </article>,
    host,
  );
}
