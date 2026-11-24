import type { Feature, Point } from "geojson";
import { flagName, silhouette, SILHOUETTE_LABEL, SILHOUETTE_SCALE, type Silhouette } from "./symbols";
import { finite, text, type Props } from "./data";

/** What Relic can say about an aircraft from the live feed alone, with no
 *  further request: the operator its callsign names, the state its Mode S
 *  address is allocated to, its type designator's name, what a state
 *  aircraft's type is for, and how its position was obtained. The tables are
 *  Relic's own and deliberately partial: anything they do not know gives
 *  null, never a guess. Military and government status itself is only ever
 *  the provider's own category (`military`), never inferred here. */

// ---------------------------------------------------------------- type names
// ICAO type designators as the feed sends them.
const TYPES = `A124 Antonov An-124 Ruslan|A139 Leonardo AW139|A169 Leonardo AW169|A189 Leonardo AW189|A109 Leonardo AW109|A119 Leonardo AW119|A129 Agusta A129 Mangusta
A19N Airbus A319neo|A20N Airbus A320neo|A21N Airbus A321neo|A306 Airbus A300-600|A30B Airbus A300|A310 Airbus A310|A318 Airbus A318|A319 Airbus A319|A320 Airbus A320|A321 Airbus A321
A332 Airbus A330-200|A333 Airbus A330-300|A337 Airbus BelugaXL|A338 Airbus A330-800|A339 Airbus A330-900|A342 Airbus A340-200|A343 Airbus A340-300|A345 Airbus A340-500|A346 Airbus A340-600
A359 Airbus A350-900|A35K Airbus A350-1000|A388 Airbus A380-800|A3ST Airbus Beluga|A400 Airbus A400M Atlas|A10 Fairchild A-10 Thunderbolt II|A148 Antonov An-148|AJ27 COMAC ARJ21
AN12 Antonov An-12|AN2 Antonov An-2|AN24 Antonov An-24|AN26 Antonov An-26|AN30 Antonov An-30|AN32 Antonov An-32|AN72 Antonov An-72|AN74 Antonov An-74
AS32 Airbus Super Puma|AS50 Airbus H125 Écureuil|AS55 Airbus AS355 Écureuil 2|AS65 Airbus AS365 Dauphin|ASTR IAI Astra|AT43 ATR 42-300|AT45 ATR 42-500|AT46 ATR 42-600|AT72 ATR 72|AT75 ATR 72-500|AT76 ATR 72-600
B06 Bell 206 JetRanger|B105 MBB Bo 105|B212 Bell 212|B407 Bell 407|B412 Bell 412|B429 Bell 429|B505 Bell 505|B1 Rockwell B-1 Lancer|B2 Northrop B-2 Spirit|B52 Boeing B-52 Stratofortress
B190 Beechcraft 1900|B350 Beechcraft King Air 350|B461 BAe 146-100|B462 BAe 146-200|B463 BAe 146-300|B712 Boeing 717|B703 Boeing 707
B732 Boeing 737-200|B733 Boeing 737-300|B734 Boeing 737-400|B735 Boeing 737-500|B736 Boeing 737-600|B737 Boeing 737-700|B738 Boeing 737-800|B739 Boeing 737-900
B37M Boeing 737 MAX 7|B38M Boeing 737 MAX 8|B39M Boeing 737 MAX 9|B3XM Boeing 737 MAX 10|B742 Boeing 747-200|B743 Boeing 747-300|B744 Boeing 747-400|B748 Boeing 747-8|B74S Boeing 747SP|BLCF Boeing 747 Dreamlifter
B752 Boeing 757-200|B753 Boeing 757-300|B762 Boeing 767-200|B763 Boeing 767-300|B764 Boeing 767-400|B772 Boeing 777-200|B77L Boeing 777-200LR|B773 Boeing 777-300|B77W Boeing 777-300ER|B778 Boeing 777-8|B779 Boeing 777-9
B788 Boeing 787-8|B789 Boeing 787-9|B78X Boeing 787-10|BCS1 Airbus A220-100|BCS3 Airbus A220-300|BE20 Beechcraft King Air 200|BE36 Beechcraft Bonanza|BE40 Beechjet 400|BE58 Beechcraft Baron|BE9L Beechcraft King Air 90
BK17 Kawasaki BK117|BN2P Britten-Norman Islander|C130 Lockheed C-130 Hercules|C30J Lockheed Martin C-130J Super Hercules|C160 Transall C-160|C17 Boeing C-17 Globemaster III|C5 Lockheed C-5 Galaxy|C5M Lockheed C-5M Super Galaxy
C150 Cessna 150|C152 Cessna 152|C172 Cessna 172 Skyhawk|C182 Cessna 182 Skylane|C206 Cessna 206|C208 Cessna 208 Caravan|C210 Cessna 210|C212 CASA C-212 Aviocar|C27J Leonardo C-27J Spartan|C295 Airbus C295|C310 Cessna 310
C25A Cessna Citation CJ2|C25B Cessna Citation CJ3|C25C Cessna Citation CJ4|C25M Cessna Citation M2|C414 Cessna 414|C421 Cessna 421|C510 Cessna Citation Mustang|C525 Cessna CitationJet|C550 Cessna Citation II|C560 Cessna Citation V
C56X Cessna Citation Excel|C650 Cessna Citation III|C680 Cessna Citation Sovereign|C68A Cessna Citation Latitude|C700 Cessna Citation Longitude|C750 Cessna Citation X|C919 COMAC C919|CL30 Bombardier Challenger 300|CL35 Bombardier Challenger 350|CL60 Bombardier Challenger 600
CN35 Airbus CN235|CRJ1 Bombardier CRJ100|CRJ2 Bombardier CRJ200|CRJ7 Bombardier CRJ700|CRJ9 Bombardier CRJ900|CRJX Bombardier CRJ1000|D228 Dornier 228|D328 Dornier 328|J328 Dornier 328JET|DA40 Diamond DA40|DA42 Diamond DA42|DA62 Diamond DA62|DV20 Diamond DA20
DC10 McDonnell Douglas DC-10|DH8A De Havilland Dash 8-100|DH8B De Havilland Dash 8-200|DH8C De Havilland Dash 8-300|DH8D De Havilland Dash 8-400|DHC6 De Havilland Twin Otter|E135 Embraer ERJ 135|E145 Embraer ERJ 145
E170 Embraer E170|E75L Embraer E175|E75S Embraer E175|E190 Embraer E190|E195 Embraer E195|E290 Embraer E190-E2|E295 Embraer E195-E2|E35L Embraer Legacy 600|E390 Embraer C-390 Millennium|E50P Embraer Phenom 100|E55P Embraer Phenom 300|E545 Embraer Praetor 500|E550 Embraer Praetor 600
E3CF Boeing E-3 Sentry|E3TF Boeing E-3 Sentry|E6 Boeing E-6 Mercury|E737 Boeing E-7 Wedgetail|EC20 Airbus EC120|EC25 Airbus H225|EC30 Airbus H130|EC35 Airbus H135|EC45 Airbus H145|EC55 Airbus EC155|EC75 Airbus H175|EH10 Leonardo AW101 Merlin|EN48 Enstrom 480
EUFI Eurofighter Typhoon|EXPL MD Explorer|F100 Fokker 100|F15 Boeing F-15 Eagle|F16 Lockheed Martin F-16|F18 Boeing F/A-18 Hornet|F22 Lockheed Martin F-22 Raptor|F35 Lockheed Martin F-35 Lightning II|F50 Fokker 50|F70 Fokker 70|F2TH Dassault Falcon 2000|F900 Dassault Falcon 900
FA10 Dassault Falcon 10|FA20 Dassault Falcon 20|FA50 Dassault Falcon 50|FA6X Dassault Falcon 6X|FA7X Dassault Falcon 7X|FA8X Dassault Falcon 8X|G150 Gulfstream G150|G280 Gulfstream G280|GA5C Gulfstream G500|GA6C Gulfstream G600|GA7C Gulfstream G700|GA8C Gulfstream G800
GAZL Aérospatiale Gazelle|GL5T Bombardier Global 5000|GL7T Bombardier Global 7500|GLEX Bombardier Global Express|GLF3 Gulfstream III|GLF4 Gulfstream IV|GLF5 Gulfstream V|GLF6 Gulfstream G650|GRIF Saab Gripen|GRIP Saab Gripen
H160 Airbus H160|H25B Hawker 800|H25C Hawker 1000|H47 Boeing CH-47 Chinook|H500 MD 500|H53 Sikorsky CH-53|H60 Sikorsky H-60 Black Hawk|H64 Boeing AH-64 Apache|HAWK BAE Systems Hawk|HDJT HondaJet|HRON IAI Heron
IL18 Ilyushin Il-18|IL62 Ilyushin Il-62|IL76 Ilyushin Il-76|IL78 Ilyushin Il-78|IL96 Ilyushin Il-96|JS32 BAe Jetstream 32|JS41 BAe Jetstream 41|K35E Boeing KC-135E Stratotanker|K35R Boeing KC-135R Stratotanker
L39 Aero L-39 Albatros|L410 Let L-410 Turbolet|LJ35 Learjet 35|LJ45 Learjet 45|LJ60 Learjet 60|LJ75 Learjet 75|LYNX Westland Lynx|M28 PZL M28 Skytruck|M346 Leonardo M-346|MD11 McDonnell Douglas MD-11|MD82 McDonnell Douglas MD-82|MD83 McDonnell Douglas MD-83|MD88 McDonnell Douglas MD-88
MI8 Mil Mi-8|MI24 Mil Mi-24|MIR2 Dassault Mirage 2000|NH90 NHIndustries NH90|P180 Piaggio P.180 Avanti|P28A Piper Cherokee|P28R Piper Arrow|P3 Lockheed P-3 Orion|P8 Boeing P-8 Poseidon|P46T Piper Malibu Meridian|PA31 Piper Navajo|PA34 Piper Seneca|PA46 Piper Malibu
PC12 Pilatus PC-12|PC21 Pilatus PC-21|PC24 Pilatus PC-24|PC6 Pilatus PC-6 Porter|PC7 Pilatus PC-7|PC9 Pilatus PC-9|PRM1 Beechcraft Premier I|Q4 Northrop Grumman RQ-4 Global Hawk|Q9 General Atomics MQ-9 Reaper|R135 Boeing RC-135|R22 Robinson R22|R44 Robinson R44|R66 Robinson R66
RFAL Dassault Rafale|RJ1H Avro RJ100|RJ85 Avro RJ85|S22T Cirrus SR22T|S61 Sikorsky S-61|S76 Sikorsky S-76|S92 Sikorsky S-92|SB20 Saab 2000|SF34 Saab 340|SF50 Cirrus Vision Jet|SR20 Cirrus SR20|SR22 Cirrus SR22|SU95 Sukhoi Superjet 100|SW4 Fairchild Metro
T154 Tupolev Tu-154|T204 Tupolev Tu-204|T38 Northrop T-38 Talon|TBM7 Daher TBM 700|TBM8 Daher TBM 850|TBM9 Daher TBM 900|TEX2 Beechcraft T-6 Texan II|TIGR Airbus Tiger|TOR Panavia Tornado|U2 Lockheed U-2|UH1 Bell UH-1 Iroquois|V22 Bell Boeing V-22 Osprey`;
const TYPE_NAME = new Map(TYPES.split(/[|\n]/).map(s => { const i = s.indexOf(" "); return [s.slice(0, i), s.slice(i + 1)] as [string, string]; }));
/** The name of an ICAO type designator, or null when Relic's table lacks it. */
export const typeName = (code: unknown) => typeof code === "string" ? TYPE_NAME.get(code.trim().toUpperCase()) ?? null : null;

// ---------------------------------------------------------------- operators
// ICAO three-letter airline designators, the first letters of a callsign.
const AIRLINES = `AAL American Airlines|DAL Delta Air Lines|UAL United Airlines|SWA Southwest Airlines|JBU JetBlue|ASA Alaska Airlines|NKS Spirit Airlines|FFT Frontier Airlines|HAL Hawaiian Airlines|SKW SkyWest|ENY Envoy Air|RPA Republic Airways
ACA Air Canada|WJA WestJet|JZA Jazz|TSC Air Transat|AMX Aeroméxico|VOI Volaris|AVA Avianca|LAN LATAM|TAM LATAM Brasil|GLO Gol|AZU Azul|CMP Copa Airlines|ARG Aerolíneas Argentinas
BAW British Airways|VIR Virgin Atlantic|EZY easyJet|EJU easyJet Europe|EZS easyJet Switzerland|RYR Ryanair|RUK Ryanair UK|LDM Lauda Europe|WZZ Wizz Air|WUK Wizz Air UK|AFR Air France|KLM KLM|DLH Lufthansa|CLH Lufthansa CityLine|EWG Eurowings
SWR Swiss|AUA Austrian Airlines|BEL Brussels Airlines|SAS Scandinavian Airlines|NOZ Norwegian|NSZ Norwegian|FIN Finnair|IBE Iberia|IBS Iberia Express|VLG Vueling|AEA Air Europa|TAP TAP Air Portugal|ITY ITA Airways|EIN Aer Lingus|LOT LOT Polish Airlines
CSA Czech Airlines|ROT TAROM|AEE Aegean Airlines|THY Turkish Airlines|PGT Pegasus|SXS SunExpress|AFL Aeroflot|SBI S7 Airlines|ICE Icelandair|TOM TUI Airways|TFL TUI fly Netherlands|TUI TUIfly|CFG Condor|EXS Jet2|CFE BA CityFlyer|LOG Loganair|BCY CityJet
UAE Emirates|QTR Qatar Airways|ETD Etihad Airways|SVA Saudia|FDB flydubai|ABY Air Arabia|GFA Gulf Air|OMA Oman Air|KAC Kuwait Airways|RJA Royal Jordanian|MEA Middle East Airlines|ELY El Al|IRA Iran Air|MSR EgyptAir|RAM Royal Air Maroc|ETH Ethiopian Airlines|KQA Kenya Airways|SAA South African Airways
AIC Air India|IGO IndiGo|SEJ SpiceJet|AXB Air India Express|CPA Cathay Pacific|CCA Air China|CES China Eastern|CSN China Southern|CHH Hainan Airlines|CXA Xiamen Airlines|CSZ Shenzhen Airlines|JAL Japan Airlines|ANA All Nippon Airways|KAL Korean Air|AAR Asiana Airlines
SIA Singapore Airlines|TGW Scoot|MAS Malaysia Airlines|AXM AirAsia|THA Thai Airways|VJC VietJet Air|HVN Vietnam Airlines|PAL Philippine Airlines|CEB Cebu Pacific|GIA Garuda Indonesia|QFA Qantas|VOZ Virgin Australia|JST Jetstar|ANZ Air New Zealand|EVA EVA Air|CAL China Airlines|PIA Pakistan International
FDX FedEx|UPS UPS Airlines|GTI Atlas Air|CLX Cargolux|DHK DHL Air UK|BCS European Air Transport (DHL)|ABW AirBridgeCargo|CKS Kalitta Air|ABX ABX Air|GEC Lufthansa Cargo|BOX AeroLogic|NCA Nippon Cargo Airlines|MPH Martinair|TAY ASL Airlines Belgium
NJE NetJets Europe|EJA NetJets|VJT VistaJet|LXJ Flexjet`;
const AIRLINE = new Map(AIRLINES.split(/[|\n]/).map(s => [s.slice(0, 3), s.slice(4)] as [string, string]));
/** State operators named by their callsign prefix, with the state's code.
 *  The prefix must be followed by digits, as in a flight's callsign. */
const STATE_CALLSIGNS: [string, string, string][] = [
  ["RCH", "US Air Force Air Mobility Command", "US"], ["SAM", "US Air Force Special Air Mission", "US"], ["SPAR", "US Air Force", "US"], ["FORTE", "US Air Force", "US"],
  ["CNV", "US Navy", "US"], ["PAT", "US Army", "US"], ["RRR", "Royal Air Force", "GB"], ["CFC", "Royal Canadian Air Force", "CA"],
  ["GAF", "German Air Force", "DE"], ["GAM", "German Army", "DE"], ["GNY", "German Navy", "DE"], ["CTM", "French Air and Space Force", "FR"], ["FAF", "French Air and Space Force", "FR"], ["FNY", "French Navy", "FR"],
  ["IAM", "Italian Air Force", "IT"], ["AME", "Spanish Air and Space Force", "ES"], ["PLF", "Polish Air Force", "PL"], ["HAF", "Hellenic Air Force", "GR"], ["BAF", "Belgian Air Component", "BE"],
  ["NAF", "Royal Netherlands Air Force", "NL"], ["DAF", "Royal Danish Air Force", "DK"], ["NOW", "Royal Norwegian Air Force", "NO"], ["SVF", "Swedish Air Force", "SE"], ["SUI", "Swiss Air Force", "CH"],
  ["CEF", "Czech Air Force", "CZ"], ["IRL", "Irish Air Corps", "IE"], ["ASY", "Royal Australian Air Force", "AU"], ["KIWI", "Royal New Zealand Air Force", "NZ"], ["RFF", "Russian Air Force", "RU"],
  ["NATO", "NATO", "NATO"], ["MMF", "NATO Multinational MRTT Fleet", "NATO"],
];
export type Operator = { name: string; basis: "callsign" | "flight number"; prefix: string; country: string | null };
/** The operator a callsign (or flight number) names: a state service first,
 *  then an airline. */
export function operatorOf(callsign: unknown, basis: Operator["basis"] = "callsign"): Operator | null {
  const c = typeof callsign === "string" ? callsign.trim().toUpperCase() : "";
  for (const [prefix, name, country] of STATE_CALLSIGNS) if (c.startsWith(prefix) && /^\d/.test(c.slice(prefix.length))) return { name, basis, prefix, country };
  const m = /^([A-Z]{3})\d/.exec(c);
  const airline = m ? AIRLINE.get(m[1]) : undefined;
  return airline ? { name: airline, basis, prefix: m![1], country: null } : null;
}

// ---------------------------------------------------------------- Mode S
// ICAO Annex 10 allocations of 24-bit addresses to states of registry.
// Order matters only where a block lies inside a larger one (Hong Kong).
const BLOCKS = `004000 0043FF ZW|006000 006FFF MZ|008000 00FFFF ZA|010000 017FFF EG|018000 01FFFF LY|020000 027FFF MA|028000 02FFFF TN|030000 0303FF BW|032000 032FFF BI|034000 034FFF CM|036000 036FFF CG|038000 038FFF CI|03E000 03EFFF GA
040000 040FFF ET|042000 042FFF GQ|044000 044FFF GH|046000 046FFF GN|04C000 04CFFF KE|050000 050FFF LR|054000 054FFF MG|058000 058FFF MW|05C000 05CFFF ML|060000 0603FF MU|062000 062FFF NE|064000 064FFF NG|068000 068FFF UG|06A000 06A3FF QA
06C000 06CFFF CF|06E000 06EFFF RW|070000 070FFF SN|076000 0763FF SL|078000 078FFF SO|07C000 07CFFF SD|080000 080FFF TZ|084000 084FFF TD|088000 088FFF TG|08A000 08AFFF ZM|08C000 08CFFF CD|090000 090FFF AO|098000 0983FF DJ|0A0000 0A7FFF DZ
0A8000 0A8FFF BS|0AC000 0ACFFF CO|0AE000 0AEFFF CR|0B0000 0B0FFF CU|0B2000 0B2FFF SV|0B4000 0B4FFF GT|0B8000 0B8FFF HT|0BA000 0BAFFF HN|0BE000 0BEFFF JM|0C0000 0C0FFF NI|0C2000 0C2FFF PA|0C4000 0C4FFF DO|0C6000 0C6FFF TT|0D0000 0D7FFF MX|0D8000 0DFFFF VE
100000 1FFFFF RU|300000 33FFFF IT|340000 37FFFF ES|380000 3BFFFF FR|3C0000 3FFFFF DE|400000 43FFFF GB|440000 447FFF AT|448000 44FFFF BE|450000 457FFF BG|458000 45FFFF DK|460000 467FFF FI|468000 46FFFF GR|470000 477FFF HU|478000 47FFFF NO
480000 487FFF NL|488000 48FFFF PL|490000 497FFF PT|498000 49FFFF CZ|4A0000 4A7FFF RO|4A8000 4AFFFF SE|4B0000 4B7FFF CH|4B8000 4BFFFF TR|4C0000 4C7FFF RS|4C8000 4C83FF CY|4CA000 4CAFFF IE|4CC000 4CCFFF IS|4D0000 4D03FF LU|4D2000 4D23FF MT
501C00 501FFF HR|502C00 502FFF LV|503C00 503FFF LT|504C00 504FFF MD|505C00 505FFF SK|506C00 506FFF SI|508000 50FFFF UA|510000 5103FF BY|511000 5113FF EE|513000 5133FF BA|514000 5143FF GE|516000 5163FF ME|600000 6003FF AM|600800 600BFF AZ|683000 6833FF KZ
700000 700FFF AF|702000 702FFF BD|704000 704FFF MM|706000 706FFF KW|70A000 70AFFF NP|70C000 70C3FF OM|710000 717FFF SA|718000 71FFFF KR|720000 727FFF KP|728000 72FFFF IQ|730000 737FFF IR|738000 73FFFF IL|740000 747FFF JO|748000 74FFFF LB
750000 757FFF MY|758000 75FFFF PH|760000 767FFF PK|768000 76FFFF SG|770000 777FFF LK|778000 77FFFF SY|789000 789FFF HK|780000 7BFFFF CN|7C0000 7FFFFF AU|800000 83FFFF IN|840000 87FFFF JP|880000 887FFF TH|888000 88FFFF VN|890000 890FFF YE|894000 894FFF BH
896000 896FFF AE|898000 898FFF PG|899000 8993FF TW|8A0000 8A7FFF ID|A00000 AFFFFF US|C00000 C3FFFF CA|C80000 C87FFF NZ|C88000 C88FFF FJ|E00000 E3FFFF AR|E40000 E7FFFF BR|E80000 E80FFF CL|E84000 E84FFF EC|E88000 E88FFF PY|E8C000 E8CFFF PE|E90000 E90FFF UY|E94000 E94FFF BO`;
const BLOCK = BLOCKS.split(/[|\n]/).map(s => { const [a, b, code] = s.split(" "); return [parseInt(a, 16), parseInt(b, 16), code] as [number, number, string]; });
export type Country = { code: string; name: string };
const country = (code: string): Country => ({ code, name: code === "NATO" ? "NATO" : flagName(code) ?? code });
/** The state a Mode S address is allocated to, or null outside Relic's table. */
export function registeredIn(icaoHex: unknown): Country | null {
  const n = typeof icaoHex === "string" && /^[0-9A-Fa-f]{6}$/.test(icaoHex) ? parseInt(icaoHex, 16) : NaN;
  if (!Number.isFinite(n) || n === 0) return null;
  const hit = BLOCK.find(([a, b]) => n >= a && n <= b);
  return hit ? country(hit[2]) : null;
}

// ---------------------------------------------------------------- roles
export type Role = "airlift" | "tanker" | "tanker_transport" | "aew" | "reconnaissance" | "patrol" | "fighter" | "bomber" | "trainer" | "helicopter" | "uncrewed" | "transport" | "ground";
export const ROLE_LABEL: Record<Role, string> = {
  airlift: "Airlift", tanker: "Tanker", tanker_transport: "Tanker or transport", aew: "Airborne early warning", reconnaissance: "Reconnaissance", patrol: "Maritime patrol",
  fighter: "Fast jet", bomber: "Bomber", trainer: "Trainer", helicopter: "Helicopter", uncrewed: "Uncrewed", transport: "Transport", ground: "Ground vehicle",
};
/** A count of aircraft in a role, in words: "1 fast jet", "7 helicopters". */
const ROLE_COUNT: Record<Role, [string, string]> = {
  airlift: ["airlifter", "airlifters"], tanker: ["tanker", "tankers"], tanker_transport: ["tanker or transport", "tankers or transports"], aew: ["early-warning aircraft", "early-warning aircraft"],
  reconnaissance: ["reconnaissance aircraft", "reconnaissance aircraft"], patrol: ["maritime patrol aircraft", "maritime patrol aircraft"], fighter: ["fast jet", "fast jets"], bomber: ["bomber", "bombers"],
  trainer: ["trainer", "trainers"], helicopter: ["helicopter", "helicopters"], uncrewed: ["uncrewed aircraft", "uncrewed aircraft"], transport: ["transport", "transports"], ground: ["ground vehicle", "ground vehicles"],
};
export const roleCount = (role: Role, n: number) => `${n} ${ROLE_COUNT[role][n === 1 ? 0 : 1]}`;
const ROLES: [Role, string][] = [
  ["airlift", "C17 C5 C5M C130 C30J A400 C160 C27J C295 CN35 IL76 A124 AN12 AN26 AN32 AN72 E390 C212"],
  ["tanker", "K35R K35E IL78"],
  ["tanker_transport", "A332 A333 A339 B762 B763 DC10 B703"],
  ["aew", "E3TF E3CF E737"],
  ["reconnaissance", "R135 E6 U2"],
  ["patrol", "P8 P3"],
  ["fighter", "F16 F15 F18 F22 F35 EUFI RFAL TOR GRIF GRIP MIR2 A10"],
  ["bomber", "B52 B1 B2"],
  ["trainer", "PC21 PC9 PC7 HAWK M346 TEX2 T38 L39"],
  ["uncrewed", "Q4 Q9 HRON"],
  // Airliner types flown by state operators carry people and cargo.
  ["transport", "A319 A320 A321 A19N A20N A21N A359 A35K B737 B738 B739 B752 B744 B748"],
];
const ROLE = new Map(ROLES.flatMap(([role, codes]) => codes.split(" ").map(c => [c, role] as [string, Role])));
/** What a military or government aircraft's type is for; null for others
 *  and when neither the type nor the silhouette says (no role is guessed
 *  for business jets or light aircraft, which serve many purposes). */
export function roleOf(type: unknown, shape: Silhouette, state: boolean): Role | null {
  if (!state) return null;
  const t = typeof type === "string" ? type.trim().toUpperCase() : "";
  if (ROLE.has(t)) return ROLE.get(t)!;
  if (shape === "heli") return "helicopter";
  if (shape === "drone") return "uncrewed";
  if (shape === "ground") return "ground";
  if (shape === "fighter") return "fighter";
  return null;
}

// ---------------------------------------------------------------- sources
// The feed's position source; ADS-B and MLAT are confirmed on the wire, the
// rest follow the provider's declaration order.
const SOURCES = ["ADS-B", "MLAT", "FLARM", "FAA", "Estimated", "Satellite", "Other source", "UAT", "Spidertracks", "Australian ADS-B", "Space-based ADS-B"];
export const sourceLabel = (n: unknown) => { const i = finite(n); return i != null && SOURCES[i] ? SOURCES[i] : null; };
/** Estimated positions are not measured; they are said as such. */
export const estimated = (n: unknown) => finite(n) === 4;

// ---------------------------------------------------------------- the whole picture
export type Described = { operator: Operator | null; country: Country | null; typeName: string | null; role: Role | null; source: string | null };
/** Everything above for one aircraft's properties. */
export function describe(p: Props): Described {
  const shape = (p.shape as Silhouette) ?? silhouette(Number(p.icon ?? 0), p.aircraft_type as string | null);
  // A state flight's number is often its service callsign (GNY610 for PCT610).
  const operator = operatorOf(p.callsign) ?? operatorOf(p.flight_number, "flight number");
  const own = operator?.country ? country(operator.country) : null;
  return { operator, country: own ?? registeredIn(p.icao_hex), typeName: typeName(p.aircraft_type), role: roleOf(p.aircraft_type, shape, p.military === true), source: sourceLabel(p.source) };
}
/** Map and card properties for a flight outside the viewport feed (a
 *  most-tracked row): the silhouette, size and tone the map would give it. */
export function aircraftProps(row: Props): Props {
  const shape = silhouette(Number(row.icon ?? 0), row.aircraft_type as string | null);
  return { ...row, shape, scale: SILHOUETTE_SCALE[shape], tone: row.military === true ? "military" : shape === "ground" ? "ground" : "civil" };
}
export const shapeLabel = (p: Props) => SILHOUETTE_LABEL[(p.shape as Silhouette) ?? "narrow"];
/** The best short name for an aircraft: callsign, flight, registration, type. */
export const aircraftName = (p: Props) => text(p.callsign) ?? text(p.flight_number) ?? text(p.registration) ?? text(p.description) ?? text(p.aircraft_type) ?? "Unknown callsign";

// ---------------------------------------------------------------- climb and trails
/** Climb or descent in feet per minute, rounded to 100; below this it is level. */
export const LEVEL_FPM = 300;
export const climb = (fpm: unknown) => { const v = finite(fpm); return v == null ? null : Math.abs(v) < LEVEL_FPM ? "level" : v > 0 ? "climbing" : "descending"; };
export const fpmText = (fpm: unknown) => { const v = finite(fpm); return v == null ? null : Math.abs(v) < LEVEL_FPM ? "Level" : `${v > 0 ? "+" : "−"}${(Math.round(Math.abs(v) / 100) * 100).toLocaleString("en-GB")} ft/min`; };

type Fix = { t: number; lon: number; lat: number; alt: number | null };
/** How long and how many positions a trail keeps, and the longest gap it joins. */
const TRAIL_MS = 180_000, TRAIL_FIXES = 24, GAP_MS = 120_000;
/** Recent positions of the aircraft Relic has drawn, from successive
 *  refreshes (and each flight's first buffered point). They give each
 *  aircraft a short trail and a climb rate between its last two reports
 *  at least 8 s apart. Nothing here is interpolated; gaps reset a trail. */
export class AircraftHistory {
  private fixes = new Map<string, { fixes: Fix[]; vs: number | null; seen: number }>();
  /** Records each feature's report, sets its `vs_fpm`, and returns the trails. */
  update(features: Feature[], now = Date.now()): Feature[] {
    const trails: Feature[] = [];
    for (const f of features) {
      const p = (f.properties ?? {}) as Props;
      const key = String(p.flight_id ?? ""); const t = finite(p.observed_at_ms);
      const [lon, lat] = (f.geometry as Point).coordinates;
      if (!key || t == null) continue;
      let h = this.fixes.get(key);
      const last = h?.fixes[h.fixes.length - 1];
      const fix: Fix = { t, lon, lat, alt: p.on_ground ? null : finite(p.altitude_ft) };
      // A long gap or an impossible jump starts the trail again.
      // Implied speed in knots (a degree of latitude is 60 nautical miles).
      const jump = last ? Math.hypot((lon - last.lon) * Math.cos(lat * Math.PI / 180), lat - last.lat) * 60 / (Math.max(1_000, t - last.t) / 3_600_000) > 1_500 : false;
      if (!h || !last || t - last.t > GAP_MS || jump) {
        const from = Array.isArray(p.trail_from) ? p.trail_from.map(Number) : null;
        h = { fixes: from && from.every(Number.isFinite) && from[2] < t && t - from[2] < 30_000 ? [{ lon: from[0], lat: from[1], t: from[2], alt: null }] : [], vs: null, seen: now };
        this.fixes.set(key, h);
      }
      if (!h.fixes.length || t > h.fixes[h.fixes.length - 1].t) {
        h.fixes.push(fix);
        while (h.fixes.length > TRAIL_FIXES || (h.fixes.length > 2 && t - h.fixes[0].t > TRAIL_MS)) h.fixes.shift();
        const before = [...h.fixes].reverse().find(x => x.alt != null && t - x.t >= 8_000 && t - x.t <= GAP_MS);
        h.vs = fix.alt != null && before?.alt != null ? (fix.alt - before.alt) / ((t - before.t) / 60_000) : p.on_ground ? null : h.vs;
      }
      h.seen = now;
      p.vs_fpm = h.vs == null ? null : Math.round(h.vs);
      if (h.fixes.length > 1) trails.push({ type: "Feature", properties: { tone: p.tone, flight_id: p.flight_id }, geometry: { type: "LineString", coordinates: h.fixes.map(x => [x.lon, x.lat]) } });
    }
    for (const [key, h] of this.fixes) if (now - h.seen > TRAIL_MS) this.fixes.delete(key);
    return trails;
  }
  clear() { this.fixes.clear(); }
}

// ---------------------------------------------------------------- who is up
export type PictureRow = { code: string; name: string; count: number; roles: [Role, number][]; aircraft: Props[] };
/** Military and government aircraft in view, grouped by the state their
 *  callsign or Mode S address names; aircraft nothing names come last. One
 *  live frame: it says who is up, never whether that is more than usual. */
export function militaryPicture(features: Feature[]): { total: number; rows: PictureRow[] } {
  const rows = new Map<string, PictureRow>();
  let total = 0;
  for (const f of features) {
    const p = (f.properties ?? {}) as Props;
    if (p.military !== true) continue;
    total += 1;
    const d = describe(p);
    const code = d.country?.code ?? "";
    const row = rows.get(code) ?? { code, name: d.country?.name ?? "Not attributed", count: 0, roles: [], aircraft: [] };
    row.count += 1; row.aircraft.push(p);
    const role = d.role ?? null;
    if (role) { const r = row.roles.find(([x]) => x === role); if (r) r[1] += 1; else row.roles.push([role, 1]); }
    rows.set(code, row);
  }
  const list = [...rows.values()];
  for (const r of list) { r.roles.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])); r.aircraft.sort((a, b) => aircraftName(a).localeCompare(aircraftName(b))); }
  list.sort((a, b) => (!a.code !== !b.code ? (a.code ? -1 : 1) : b.count - a.count || a.name.localeCompare(b.name)));
  return { total, rows: list };
}
