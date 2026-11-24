import { ArrowRightLeft } from "lucide-react";

export type ShipSource = "keelwatch" | "marinetraffic";
type Health = { state: string; detail?: string | null } | undefined;

/** A provider's state as a dot: answering, needing attention, or not set up. */
const tone = (h: Health) => !h ? "unknown" : h.state === "ok" ? "ok" : h.state === "not_configured" ? "off" : h.state === "unknown" ? "unknown" : "degraded";
/** KeelWatch cannot draw ships now: not configured, unreachable or reporting a
 *  problem, or its tiles are failing in this view. */
export const keelwatchDown = (h: Health, tilesFailing: boolean) => tilesFailing || (!!h && ["degraded", "not_configured"].includes(h.state));

/** Where ships come from: KeelWatch (collected, with history) or
 *  MarineTraffic's public live map. When KeelWatch cannot draw ships, the
 *  MarineTraffic fallback is offered prominently. Remembered per browser. */
export default function ShipSourceControl({ source, onSource, keelwatch, marinetraffic, tilesFailing, past, onLive }: {
  source: ShipSource; onSource: (s: ShipSource) => void; keelwatch: Health; marinetraffic: Health; tilesFailing: boolean; past: string | null; onLive: (() => void) | null;
}) {
  // Unknown until the health answer arrives: offered, never shown as unconfigured meanwhile.
  const mtReady = marinetraffic?.state !== "not_configured";
  const offer = source === "keelwatch" && mtReady && keelwatchDown(keelwatch, tilesFailing);
  const option = (id: ShipSource, label: string, hint: string, health: Health, disabled = false) => <button aria-pressed={source === id} disabled={disabled}
    title={disabled ? "Set MARINETRAFFIC_PROXY_URLS in the private configuration" : health?.detail ?? undefined} onClick={() => onSource(id)}>
    <i className={`live-dot ${tone(health)}`} aria-hidden/><span><strong>{label}</strong><small>{hint}</small></span>
  </button>;
  return <>
    <div className="live-source" role="group" aria-label="Ship source">
      {option("keelwatch", "KeelWatch", "collected · history", keelwatch)}
      {option("marinetraffic", "MarineTraffic", mtReady ? "live map" : "not configured", marinetraffic, !mtReady)}
    </div>
    {offer && <div className="live-fallback" role="note">
      <p><strong>{keelwatch?.state === "not_configured" ? "KeelWatch is not configured" : /unreachable|did not answer/i.test(keelwatch?.detail ?? "") ? "KeelWatch is not answering" : tilesFailing ? "KeelWatch ship tiles are failing" : "KeelWatch needs attention"}</strong>
        <span>MarineTraffic can show the live ships in view instead.</span></p>
      <button onClick={() => onSource("marinetraffic")}><ArrowRightLeft size={13}/>Use MarineTraffic</button>
    </div>}
    {source === "marinetraffic" && (past
      ? <div className="live-fallback past" role="note">
          <p><strong>MarineTraffic is live only</strong><span>Relic keeps no MarineTraffic history, so ships at a past moment come from KeelWatch.</span></p>
          <span className="live-fallback-actions"><button onClick={() => onSource("keelwatch")}>Use KeelWatch</button>{onLive && <button onClick={onLive}>Back to live</button>}</span>
        </div>
      : <p className="live-note">Live positions from MarineTraffic's public map, through Relic's proxies. Zoomed out it sends a sample; zoom in for more ships. No history: past moments, tracks and the ship finder use KeelWatch.</p>)}
  </>;
}
