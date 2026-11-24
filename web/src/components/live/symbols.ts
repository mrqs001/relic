import type maplibregl from "maplibre-gl";

/** Map symbology for the live layers: one colour per ship category, aircraft
 *  silhouettes by class, and red for military on both layers. */

export const SHIP_CATEGORIES = ["cargo", "tanker", "passenger", "high_speed", "fishing", "service", "leisure", "military", "navaid", "other"] as const;
export type ShipCategory = typeof SHIP_CATEGORIES[number];
export const SHIP_COLOR: Record<ShipCategory, string> = {
  cargo: "#62c98b", tanker: "#e9a23b", passenger: "#5aa6ff", high_speed: "#f3dd4b", fishing: "#ff8f70",
  service: "#3fd0c9", leisure: "#d58cf4", military: "#ff4f4f", navaid: "#d9e2e8", other: "#8fa1ae",
};
export const SHIP_LABEL: Record<ShipCategory, string> = {
  cargo: "Cargo", tanker: "Tankers", passenger: "Passenger", high_speed: "High-speed", fishing: "Fishing",
  service: "Tugs and service", leisure: "Pleasure craft", military: "Military", navaid: "Navigation aids", other: "Other",
};
/** KeelWatch tiles name the category `cat`. */
export const shipColor: maplibregl.ExpressionSpecification = ["match", ["get", "cat"], ...SHIP_CATEGORIES.flatMap(c => [c, SHIP_COLOR[c]]), SHIP_COLOR.other] as unknown as maplibregl.ExpressionSpecification;

/** Movement from the reported status and speed. */
export type Movement = "moving" | "anchored" | "moored" | "unknown";
export function movement(nav: string | null | undefined, sog: number | null | undefined): Movement {
  const s = (nav ?? "").toLowerCase();
  if (s.includes("anchor")) return "anchored";
  if (s.includes("moored") || s.includes("aground")) return "moored";
  if (sog != null && sog >= 0.5) return "moving";
  if (s.includes("under way") || s.includes("underway")) return sog == null ? "moving" : "moored";
  return sog != null ? "moored" : "unknown";
}
export const MOVEMENT_LABEL: Record<Movement, string> = { moving: "Under way", anchored: "At anchor", moored: "Moored or stopped", unknown: "Status unknown" };

/** Aircraft service groups: Flightradar24's own categories, in filter chips. */
export const AIRCRAFT_GROUPS = [
  { key: "airliners", label: "Airliners and cargo", services: [0, 1] },
  { key: "military", label: "Military and government", services: [2] },
  { key: "business", label: "Business jets", services: [3] },
  { key: "light", label: "Light aircraft", services: [4] },
  { key: "helicopters", label: "Helicopters", services: [5] },
  { key: "other", label: "Gliders, balloons and drones", services: [6, 7, 8, 10, 11] },
  { key: "ground", label: "Ground vehicles", services: [9] },
] as const;
export type AircraftGroup = typeof AIRCRAFT_GROUPS[number]["key"];

export type Silhouette = "narrow" | "wide" | "heavy" | "bizjet" | "turboprop" | "light" | "heli" | "fighter" | "drone" | "balloon" | "ground";
const HELICOPTERS = new Set(["H60", "H64", "H47", "H53", "EC20", "EC25", "EC30", "EC35", "EC45", "EC55", "EC75", "AS32", "AS3B", "AS50", "AS55", "AS65", "A109", "A119", "A129", "A139", "A149", "A169", "A189", "B06", "B06T", "B105", "B212", "B222", "B230", "B407", "B412", "B427", "B429", "B430", "B505", "R22", "R44", "R66", "S61", "S64", "S70", "S76", "S92", "MI8", "MI14", "MI17", "MI24", "MI26", "MI28", "KA27", "KA32", "KA52", "NH90", "TIGR", "LYNX", "EH10", "PUMA", "UH1", "UH1Y", "H1", "AH1", "V22", "H160", "H125", "H130", "H135", "H145", "H155", "H175", "H215", "H225", "EXPL", "GAZL", "ALO2", "ALO3", "ALH", "BK17", "CH47", "CH53", "HH60", "MH60", "SH60", "UH60"]);
const FIGHTERS = new Set(["F16", "F15", "F18", "F18S", "F18H", "F35", "F22", "F14", "F4", "F5", "EUFI", "RFAL", "TOR", "MIR2", "M2000", "MG29", "MG31", "SU24", "SU25", "SU27", "SU30", "SU33", "SU34", "SU35", "SU57", "J10", "J11", "J20", "JF17", "HAWK", "T38", "A10", "AV8B", "HAR", "GRIF", "JAGR", "L159", "M346", "T50", "KF21", "F2TH"]);
const DRONES = new Set(["Q1", "Q4", "Q9", "Q25", "HRON", "HERN", "RQ4", "MQ9", "MQ1", "TB2", "SHAH", "ANKA", "WLNG"]);
/** Silhouette from Flightradar24's icon class, then the ICAO type designator. */
export function silhouette(icon: number, type: string | null | undefined): Silhouette {
  const t = (type ?? "").toUpperCase();
  if (icon === 25 || t === "GRND") return "ground";
  if (icon === 24 || t === "BALL") return "balloon";
  if (HELICOPTERS.has(t)) return "heli";
  if (DRONES.has(t)) return "drone";
  if (FIGHTERS.has(t)) return "fighter";
  switch (icon) {
    case 0: case 7: case 10: case 11: return "narrow";
    case 12: case 15: case 18: return "wide";
    case 9: case 16: case 17: case 19: case 20: return "heavy";
    case 5: return "bizjet";
    case 6: return "turboprop";
    case 3: case 4: return "light";
  }
  return "narrow";
}
export const SILHOUETTE_LABEL: Record<Silhouette, string> = {
  narrow: "Narrow-body jet", wide: "Wide-body jet", heavy: "Four-engine aircraft", bizjet: "Business jet", turboprop: "Turboprop",
  light: "Light aircraft", heli: "Helicopter", fighter: "Fast jet", drone: "Uncrewed aircraft", balloon: "Balloon", ground: "Ground vehicle",
};
/** Drawn size relative to a narrow-body jet. */
export const SILHOUETTE_SCALE: Record<Silhouette, number> = { narrow: 1, wide: 1.22, heavy: 1.32, bizjet: 0.78, turboprop: 0.85, light: 0.68, heli: 0.78, fighter: 0.8, drone: 0.82, balloon: 0.62, ground: 0.5 };
export const AIRCRAFT_COLOR = { civil: "#f4c84a", military: "#ff4f4f", ground: "#8795a1" } as const;

// ---------------------------------------------------------------- value ramps
// One colour scale per measure, shared by the map path and the card's chart,
// so the profile under the card reads directly against the line on the map.
export type Ramp = { stops: [number, string][]; unit: string; label: (v: number) => string };
export const ALTITUDE_RAMP: Ramp = {
  stops: [[0, "#e9a23b"], [5_000, "#f3dd4b"], [15_000, "#62c98b"], [28_000, "#4cc9f0"], [40_000, "#b69cff"]],
  unit: "ft", label: v => v >= 18_000 ? `FL${Math.round(v / 100)}` : `${Math.round(v).toLocaleString("en-GB")} ft`,
};
export const SPEED_RAMP: Ramp = {
  stops: [[0, "#7c8f9c"], [1, "#e9a23b"], [6, "#62c98b"], [14, "#4cc9f0"], [22, "#b69cff"]],
  unit: "kn", label: v => `${v.toFixed(v < 10 ? 1 : 0)} kn`,
};
/** A map expression colouring by the feature's `v`. */
export const rampExpression = (r: Ramp): maplibregl.ExpressionSpecification => ["interpolate", ["linear"], ["get", "v"], ...r.stops.flat()] as unknown as maplibregl.ExpressionSpecification;
/** The ramp's colour at a value, for legends and charts. */
export function rampColor(r: Ramp, v: number): string {
  const s = r.stops;
  if (v <= s[0][0]) return s[0][1];
  for (let i = 1; i < s.length; i++) {
    if (v <= s[i][0]) {
      const t = (v - s[i - 1][0]) / (s[i][0] - s[i - 1][0]);
      const a = s[i - 1][1], b = s[i][1];
      const mix = (k: number) => Math.round(parseInt(a.slice(k, k + 2), 16) * (1 - t) + parseInt(b.slice(k, k + 2), 16) * t).toString(16).padStart(2, "0");
      return `#${mix(1)}${mix(3)}${mix(5)}`;
    }
  }
  return s[s.length - 1][1];
}
/** Country name from a two-letter flag code, falling back to the code. */
const regions = (() => { try { return new Intl.DisplayNames(["en-GB"], { type: "region" }); } catch { return null; } })();
export function flagName(code: unknown): string | null {
  if (typeof code !== "string" || !/^[A-Za-z]{2}$/.test(code)) return typeof code === "string" && code.trim() ? code.trim() : null;
  try { return regions?.of(code.toUpperCase()) ?? code.toUpperCase(); } catch { return code.toUpperCase(); }
}

// ---------------------------------------------------------------- icons
// 64 px signed-distance icons, nose up; the map colours and rotates them.
const S = 64;
function draw(paint: (c: CanvasRenderingContext2D) => void) {
  const canvas = document.createElement("canvas"); canvas.width = canvas.height = S;
  const c = canvas.getContext("2d")!; c.fillStyle = "#000"; c.strokeStyle = "#000"; c.lineJoin = "round"; c.lineCap = "round";
  paint(c);
  return c.getImageData(0, 0, S, S);
}
/** A closed outline from right-side points (nose to tail), mirrored left. */
function mirrored(c: CanvasRenderingContext2D, half: [number, number][]) {
  c.beginPath();
  c.moveTo(half[0][0], half[0][1]);
  for (const [x, y] of half.slice(1)) c.lineTo(x, y);
  for (const [x, y] of [...half].reverse()) c.lineTo(S - x, y);
  c.closePath(); c.fill();
}
const pod = (c: CanvasRenderingContext2D, x: number, y: number, w: number, h: number) => { c.beginPath(); c.roundRect(x - w / 2, y, w, h, w / 2); c.fill(); c.beginPath(); c.roundRect(S - x - w / 2, y, w, h, w / 2); c.fill(); };
const SHAPES: Record<Silhouette | "ship" | "ship-still" | "warning", () => ImageData> = {
  narrow: () => draw(c => { mirrored(c, [[32, 5], [34.6, 9], [35, 22], [58, 36], [58, 39.5], [35, 33], [34.4, 48], [44, 55], [44, 58], [33, 56], [32, 60]]); pod(c, 42, 30, 3.6, 7); }),
  wide: () => draw(c => { mirrored(c, [[32, 3], [35.4, 8], [36, 21], [61, 36], [61, 40], [36, 33], [35.2, 49], [46, 56], [46, 59], [33.4, 57], [32, 62]]); pod(c, 42.5, 29, 4.4, 8); }),
  heavy: () => draw(c => { mirrored(c, [[32, 2], [35.6, 7], [36.2, 20], [62, 36], [62, 40], [36.2, 33], [35.4, 49], [46, 56], [46, 59], [33.4, 57], [32, 62]]); pod(c, 41, 27, 3.8, 7.5); pod(c, 51, 32.5, 3.6, 7); }),
  bizjet: () => draw(c => { mirrored(c, [[32, 8], [34.3, 13], [34.6, 27], [52, 38], [52, 41], [34.6, 36], [34.3, 50], [37.6, 50], [37.6, 44.5], [35, 46], [35, 51], [43, 54], [43, 56.5], [32, 56]]); }),
  turboprop: () => draw(c => { mirrored(c, [[32, 7], [34.4, 12], [34.6, 25], [60, 26], [60, 32], [34.6, 32], [34, 50], [44, 52], [44, 56], [32, 57]]); pod(c, 43, 20, 4.4, 10); }),
  light: () => draw(c => { mirrored(c, [[32, 12], [34, 16], [34.4, 23], [57, 23.5], [57, 29], [34.4, 29], [33.4, 47], [41, 48], [41, 52], [32, 52.5]]); }),
  heli: () => draw(c => {
    c.beginPath(); c.ellipse(32, 29, 6.5, 10, 0, 0, Math.PI * 2); c.fill();
    c.fillRect(30.8, 36, 2.4, 20); c.fillRect(25, 53, 14, 2.6);
    c.lineWidth = 2.6; c.beginPath(); c.moveTo(10, 11); c.lineTo(54, 47); c.moveTo(54, 11); c.lineTo(10, 47); c.stroke();
  }),
  fighter: () => draw(c => { mirrored(c, [[32, 4], [34, 12], [35, 24], [55, 43], [55, 47], [36, 44], [36, 50], [44, 57], [44, 60], [33, 58], [32, 61]]); }),
  drone: () => draw(c => { mirrored(c, [[32, 10], [33.6, 14], [34, 24], [62, 26], [62, 29.5], [34, 30], [33.4, 49], [42, 57], [41, 59.5], [32, 55]]); }),
  balloon: () => draw(c => { c.beginPath(); c.arc(32, 27, 17, 0, Math.PI * 2); c.fill(); c.fillRect(28, 48, 8, 8); }),
  ground: () => draw(c => { c.beginPath(); c.roundRect(20, 14, 24, 36, 6); c.fill(); }),
  ship: () => draw(c => { c.beginPath(); c.moveTo(32, 3); c.bezierCurveTo(40, 12, 44, 22, 44, 34); c.lineTo(43, 60); c.lineTo(21, 60); c.lineTo(20, 34); c.bezierCurveTo(20, 22, 24, 12, 32, 3); c.closePath(); c.fill(); }),
  "ship-still": () => draw(c => { c.beginPath(); c.arc(32, 32, 15, 0, Math.PI * 2); c.fill(); }),
  warning: () => draw(c => { c.beginPath(); c.moveTo(32, 5); c.lineTo(59, 32); c.lineTo(32, 59); c.lineTo(5, 32); c.closePath(); c.fill(); }),
};
export const ICON_PREFIX = "live-icon-";
export function installIcons(m: maplibregl.Map) {
  for (const [name, make] of Object.entries(SHAPES)) {
    const id = ICON_PREFIX + name;
    if (!m.hasImage(id)) m.addImage(id, make(), { sdf: true, pixelRatio: 2 });
  }
}
