import { z } from "zod";

const id = z.string().min(1);
const refSchema = z.object({ evidence_id: id, revision: id });
export const citationSchema = refSchema.extend({
  start_character: z.number().int().nonnegative(),
  end_character: z.number().int().nonnegative(),
});
export type Citation = z.infer<typeof citationSchema>;
export const eventTypes = ["strike", "armed_attack", "explosion", "air_defense", "weapons_launch", "territorial_change", "military_movement", "military_activity", "maritime_interdiction", "security_alert", "alert_lifted", "evacuation", "rescue", "fire", "protest", "infrastructure_disruption", "structural_damage", "transport_disruption", "accident", "disaster", "diplomatic_meeting", "agreement", "sanctions", "policy_change", "arrest", "legal_proceeding", "election", "aid_delivery", "cyber_incident", "other"] as const;
export type EventType = typeof eventTypes[number];
export const scopeSchema = z.object({
  source_ids: z.array(id),
  include_unlocated: z.boolean(),
  query: z.string(),
  time_basis: z.enum(["publication_time", "acquisition_time"]),
  from: z.string().nullable(),
  to: z.string().nullable(),
  bbox: z.tuple([z.number(), z.number(), z.number(), z.number()]).nullable(),
  publication_id: id.nullable(),
  content_view: z.enum(["all", "signals", "events"]).default("all"),
  event_type: z.enum(eventTypes).nullable().default(null),
  related_key: z.string().nullable().default(null),
});
export type Scope = z.infer<typeof scopeSchema>;
export const emptyScope: Scope = {
  source_ids: [],
  include_unlocated: true,
  query: "",
  time_basis: "publication_time",
  from: null,
  to: null,
  bbox: null,
  publication_id: null,
  content_view: "events",
  event_type: null,
  related_key: null,
};
export const claimSchema = z.object({
  id,
  text: z.string(),
  state: z.enum(["asserted", "denied", "corrected", "unresolved", "withdrawn"]),
  citations: z.array(citationSchema),
});
export const statuses = [
  "not_assessed",
  "supported",
  "contested",
  "insufficiently_supported",
] as const;
const pointSchema = z.object({
  type: z.literal("Point"), coordinates: z.tuple([z.number(), z.number()]),
});
export const interpretationSchema = z.object({
  id, language: z.string(), translation: z.string().nullable(), headline: z.string(),
  kind: z.enum(["event", "statement", "analysis", "promotion", "other"]),
  claim_status: z.enum(["reported", "denied", "planned", "historical", "unclear"]),
  topic: z.string(),
  events: z.array(z.object({ id, type: z.enum(eventTypes), state: z.enum(["reported", "denied", "planned", "historical", "unclear"]), text: z.string(), quote: z.string(), citation: citationSchema, time_text: z.string().nullable(), locations: z.array(z.unknown()), geometry: pointSchema.nullable(), location_label: z.string().nullable(), location_precision: z.string().nullable() })).nullish().transform(events => events ?? []),
  claims: z.array(z.object({ text: z.string(), quote: z.string(), state: z.string(), citation: citationSchema })),
  locations: z.array(z.unknown()), location_label: z.string().nullable(), location_precision: z.string().nullable(),
  geometry: pointSchema.nullable(), related_count: z.number().int().nonnegative(), related_key: z.string().nullable(),
  model: z.string(), created_at: z.string(),
});
export type Interpretation = z.infer<typeof interpretationSchema>;
export const assessmentSchema = z.object({
  id,
  claim_id: id,
  status: z.enum(statuses),
  rationale: z.string(),
  citations: z.array(citationSchema),
  scope: scopeSchema,
  publication_id: id,
  created_at: z.string(),
  actor: z.string(),
});
export const evidenceSchema = z.object({
  id,
  revision: id,
  message_key: z.string().regex(/^[1-9][0-9]*$/).optional(),
  source: id,
  source_label: z.string(),
  family: z.string(),
  text: z.string(),
  published_at: z.string().nullable(),
  acquired_at: z.string(),
  source_edited_at: z.string().nullish(),
  event_at: z.string().nullish(),
  geometry: pointSchema.nullable(),
  map_areas: z.array(z.object({ id: z.string(), label: z.string(), bounds: z.tuple([z.number(), z.number(), z.number(), z.number()]), level: z.string(), country_code: z.string().nullish(), represented_time: z.string().optional() })).default([]),
  map_locations: z.array(z.object({ id: z.string(), label: z.string().nullable(), geometry: pointSchema, approximate: z.boolean() })).default([]),
  location_precision: z.string(),
  origin_group: z.string().nullable(),
  dependence: z.string().nullish(),
  copies_from: z.object({ id, revision: id }).nullish(),
  supersedes: z.object({ id, revision: id }).nullish(),
  language: z.string(),
  fictional: z.boolean(),
  availability: z.string(),
  upstream_state: z.enum(["present", "removed"]).optional(),
  media: z.array(z.unknown()).max(100).optional(),
  relationships: z.record(z.string(), z.unknown()).optional(),
  /** Issuer fields of an institutional warning, kept with its evidence. */
  advisory: z.object({ issuer: z.string(), type: z.string().nullish(), reference: z.string().nullish() }).nullish(),
  /** Other reports of this message's event within the same scope. */
  event_links: z.number().int().positive().optional(),
  /** The linked event this message belongs to in this scope, and its report count. */
  event_group: z.object({ id: z.string(), reports: z.number().int().positive() }).optional(),
  /** Jev's suggested event when the main reading found none; not a reading. */
  possible_event: z.object({ type: z.enum(eventTypes), confidence: z.number(), suggested_by: z.literal("jev") }).optional(),
  object_hash: z.string(),
  citation: citationSchema.nullable(),
  interpretation: interpretationSchema.nullish(),
});
export const evidencePreviewSchema = evidenceSchema.extend({
  interpretation: interpretationSchema.omit({claims:true,model:true,created_at:true}).extend({
    events:z.array(z.object({type:z.enum(eventTypes),state:z.enum(["reported","denied","planned","historical","unclear"]),locations:z.array(z.unknown())})).nullish().transform(events=>events??[]),
  }).nullish(),
});
export type EvidencePreview = z.infer<typeof evidencePreviewSchema>;
export type Evidence = z.infer<typeof evidenceSchema>;
const supplementalAnswer = z.object({type: z.literal("choice"), choice: z.string(), probabilities: z.record(z.string(), z.number()), confidence: z.number()});
const supplementalSchema = z.object({id, model:z.string(), mode:z.literal("supplementary_only"), phase:z.string(), created_at:z.string(), request_hash:z.string(), audit_hash:z.string(), answers:z.record(z.string(), supplementalAnswer), context:z.object({neighbours:z.array(z.object({id,revision:id,text:z.string()})), locations:z.array(z.object({mention:z.string(),candidates:z.array(z.object({id,name:z.string(),country_code:z.string().nullable()}))})), omitted_neighbours:z.number(),omitted_locations:z.number()})});
export const evidenceDetailSchema = evidenceSchema.extend({
  supplemental: supplementalSchema.nullish(),
  interpretation_publication_id: id.nullish(),
  media_context: z.object({
    members: z.array(z.object({ id, revision: id, published_at: z.string().nullable(), reason: z.enum(["original", "album", "reply", "nearby"]), media: z.array(z.unknown()) })),
    anchor: z.object({ id, revision: id, reason: z.string() }).nullable(),
    limited: z.boolean(),
  }).optional(),
  revisions: z.array(id),
  claims: z.array(claimSchema),
  lineage_root: id,
  original_input_hashes: z.array(z.string().regex(/^[a-f0-9]{64}$/)).optional(),
});
export type EvidenceDetail = z.infer<typeof evidenceDetailSchema>;
export const investigationSchema = z.object({
  id,
  title: z.string(),
  question: z.string(),
  scope: scopeSchema,
  version: z.number().int(),
  notes: z.string(),
  pins: z.array(refSchema.extend({ reason: z.string() })),
  assessments: z.array(assessmentSchema),
  claims: z.array(claimSchema),
});
export type Investigation = z.infer<typeof investigationSchema>;
export const briefSchema = z.object({
  id,
  revision: id,
  title: z.string(),
  investigation_id: id,
  question: z.string(),
  finding: z.string(),
  assessment: z.string(),
  limitations: z.array(z.string()),
  citations: z.array(citationSchema),
  scope: scopeSchema,
  publication_id: id,
  knowledge_cutoff: z.string(),
  created_at: z.string(),
  authorship: z.enum(["analyst", "fixture"]),
  unsupported: z.boolean().optional(),
  version: z.number().int(),
});
export type Brief = z.infer<typeof briefSchema>;
export const sourceSchema = z.object({
  id,
  label: z.string(),
  family: z.string(),
  provider: z.string(),
  status: z.string(),
  coverage: z.string(),
  fictional: z.boolean(),
  detail: z.string(),
});
export const subscriptionSchema = z.object({
  id, version: z.number().int(), provider: z.enum(["telegram", "x"]),
  label: z.string(), target: z.string(), mode: z.enum(["stream", "poll"]),
  poll_interval_seconds: z.number().int().nullable(), enabled: z.boolean(),
  created_at: z.string(), updated_at: z.string(),
  status: z.enum(["not_connected", "paused", "waiting", "running", "catching_up", "needs_attention", "blocked"]), status_detail: z.string(),
  collection: z.object({
    last_success_at: z.string().nullable(), next_due_at: z.string().nullable(),
    messages_acquired: z.number().int().nonnegative(), can_continue: z.boolean(),
    can_rebaseline: z.boolean().default(false), gap_recorded: z.boolean().default(false), delayed: z.boolean().default(false), stalled: z.boolean().default(false),
  }).optional(),
});
export type Subscription = z.infer<typeof subscriptionSchema>;
export const subscriptionsSchema = z.object({
  items: z.array(subscriptionSchema), total: z.number().int(), next_cursor: z.string().nullable(),
});
export const connectionSchema = z.object({
  provider: z.enum(["telegram", "x"]),
  status: z.enum(["unconfigured", "disconnected", "connecting", "connected", "awaiting_phone", "awaiting_code", "awaiting_password", "error"]),
  detail: z.string(), account_label: z.string().nullable(), retry_at: z.string().nullable(),
});
export type Connection = z.infer<typeof connectionSchema>;
export const connectionsSchema = z.object({
  providers: z.array(connectionSchema), collection_allowed: z.boolean(), workspace_detail: z.string(),
});
export type Connections = z.infer<typeof connectionsSchema>;
export const mediaReceiptSchema = z.object({
  url: z.string(), media_type: z.string(), bytes: z.number().int().nonnegative(),
  revision: z.string().optional(),
});
export const jobSchema = z.object({
  id,
  kind: z.string(),
  status: z.string(),
  accepted: z.number(),
  total: z.number(),
  created_at: z.string(),
  detail: z.string(),
});
export type Job = z.infer<typeof jobSchema>;
export const processingSchema = z.object({
  relationships:z.object({projection_errors:z.number(),enabled:z.boolean(),activation_required:z.boolean().optional(),stopped:z.boolean().optional(),completed:z.number(),in_flight:z.number(),failed:z.number(),links:z.number(),known_cost_usd:z.number(),unknown_cost_requests:z.number(),last_error:z.string().nullable(),running:z.boolean(),
    semantic:z.object({vectors:z.number(),candidates:z.number(),requests:z.number(),failed:z.number()}).optional()}).optional(),
  supplemental: z.object({concurrency:z.number().int().positive().optional(),model:z.string().optional(),enabled:z.boolean(),running:z.boolean(),completed:z.number(),text_size_excluded:z.number().default(0),request_excluded:z.number().default(0),last_error:z.string().nullable(),today:z.object({calls:z.number(),limit:z.number().nullable(),known_cost_usd:z.number(),unknown_cost_requests:z.number(),admission_reserved_usd:z.number(),spend_limit_usd:z.number().nullable()})}).optional(),
  configured: z.boolean(),
  evaluated: z.boolean().default(false),
  quality_notice: z.string().nullable().default(null),
  retry_at: z.string().nullable().default(null),
  retry_reason: z.enum(["credits", "rate_limit"]).nullish(),
  policy: z.object({ enabled: z.boolean(), eligible_source_ids: z.array(id) }),
  without_text: z.number().int().nonnegative().default(0),
  queued: z.number().int().nonnegative(), in_flight: z.number().int().nonnegative(), corrections_queued: z.number().int().nonnegative(), interrupted: z.number().int().nonnegative().default(0),
  unavailable: z.object({ removed: z.number().int().nonnegative(), without_text: z.number().int().nonnegative(), ineligible: z.number().int().nonnegative(), oversized: z.number().int().nonnegative() }),
  pending: z.number().int().nonnegative(), processed: z.number().int().nonnegative(), needs_upgrade: z.number().int().nonnegative().default(0), failed: z.number().int().nonnegative(),
  running: z.boolean(), last_error: z.string().nullable(),
  today: z.object({ calls: z.number().int().nonnegative(), limit: z.number().int().positive().nullable(), cost_usd: z.number().nullable(), unknown_cost_requests: z.number().int().nonnegative(), admission_reserved_usd: z.number().optional(), spend_limit_usd: z.number().nullable().optional(), next_request_reserve_usd: z.number().optional() }),
  limits: z.object({ batch: z.number().int().positive(), concurrency: z.number().int().positive() }),
  jobs: z.array(jobSchema),
});
export type ProcessingStatus = z.infer<typeof processingSchema>;
export const watchSchema = z.object({
  id,
  title: z.string(),
  question: z.string(),
  scope: scopeSchema,
  conditions: z.array(z.string()),
  status: z.string(),
  baseline_publication_id: id,
  created_at: z.string(),
  frequency: z.literal("unknown"),
});
export const alertSchema = z.object({
  id,
  watch_id: id,
  reason: z.string(),
  evidence: z.array(refSchema),
  origin_group: z.string().nullable(),
  first_at: z.string(),
  latest_at: z.string(),
  review_status: z.string(),
  delivery_status: z.string(),
  snoozed_until: z.string().nullish(),
});
export const workspaceSchema = z.object({
  name: z.literal("Relic"),
  empty: z.boolean(),
  fictional: z.boolean(),
  sources: z.array(sourceSchema),
  investigations: z.array(investigationSchema),
  briefs: z.array(briefSchema),
  publications: z.array(
    z.object({
      id,
      label: z.string(),
      created_at: z.string(),
      count: z.number(),
      status: z.string(),
    }),
  ),
  publications_total: z.number().int().nonnegative().optional(),
  publications_truncated: z.boolean().optional(),
  watches: z.array(watchSchema),
  jobs: z.array(jobSchema),
  coverage: z.array(z.string()),
  sequence: z.number(),
});
export type Workspace = z.infer<typeof workspaceSchema>;
const mapSchema = z.object({
  type: z.literal("FeatureCollection"),
  features: z.array(
    z.object({
      type: z.literal("Feature"),
      id: z.union([z.string(), z.number()]).optional(),
      geometry: z.object({
        type: z.literal("Point"),
        coordinates: z.array(z.number()).min(2),
      }),
      properties: z
        .object({
          evidence_id: id,
          revision: id,
          source: z.string(),
          title: z.string(),
        })
        .passthrough(),
    }),
  ),
});
export const timelineSchema = z.object({
  earlier: z.number().default(0), earliest: z.string().nullable().default(null),
  from: z.string().nullable(), to: z.string().nullable(),
  time_basis: z.enum(["publication_time", "acquisition_time"]), undated: z.number(),
  buckets: z.array(z.object({ from: z.string(), to: z.string(), count: z.number() })),
});
export type Timeline = z.infer<typeof timelineSchema>;
const areaMapSchema = z.object({
  type: z.literal("FeatureCollection"),
  features: z.array(z.object({ type: z.literal("Feature"), id: z.string(),
    geometry: z.discriminatedUnion("type", [
      z.object({ type: z.literal("Polygon"), coordinates: z.array(z.array(z.tuple([z.number(), z.number()]))) }),
      z.object({ type: z.literal("MultiPolygon"), coordinates: z.array(z.array(z.array(z.tuple([z.number(), z.number()])))) }),
    ]),
    properties: z.object({ area_id: z.string(), label: z.string(), level: z.string(), country_code: z.string().nullish(), bounds: z.tuple([z.number(),z.number(),z.number(),z.number()]), message_count: z.number(), choices: z.array(z.object({id,revision:id,source:z.string(),title:z.string(),published_at:z.string().nullish(),acquired_at:z.string().optional(),events:z.array(z.object({type:z.enum(eventTypes),state:z.enum(["reported","denied","planned","historical","unclear"])})).default([]),processed:z.boolean().optional()})) }),
  })),
});
export const querySchema = z.object({
  scope_revision: id,
  scope: scopeSchema,
  publication_id: id,
  generation: z.number(),
  total: z.number(),
  located: z.number(),
  unlocated: z.number(),
  unread: z.number().default(0),
  not_selected: z.number().default(0),
  media_only: z.number().default(0),
  origin_groups: z.number(),
  unknown_origins: z.number(),
  loaded: z.number(),
  items: z.array(evidenceSchema),
  map: mapSchema,
  map_areas: areaMapSchema.default({type:"FeatureCollection",features:[]}),
  areas_truncated: z.boolean().default(false),
  map_loaded: z.number(),
  map_truncated: z.boolean().default(false),
  next_cursor: z.string().nullable(),
  coverage: z.array(z.string()),
  timeline: timelineSchema,
});
export const queryViewSchema = querySchema.extend({items:z.array(evidencePreviewSchema)});
export type QueryResult = z.infer<typeof queryViewSchema>;
export const queryPageSchema = queryViewSchema.pick({publication_id:true,scope_revision:true,generation:true,scope:true,loaded:true,items:true,next_cursor:true});
const shapeGeometrySchema = areaMapSchema.shape.features.element.shape.geometry;
export const shapeSchema = z.object({id:z.string(),geometry:shapeGeometrySchema});
export const queryPreviewSchema = queryViewSchema.extend({map_areas:areaMapSchema.extend({
  features:z.array(areaMapSchema.shape.features.element.extend({geometry:shapeGeometrySchema.nullable()})),
})});
export const lineageSchema = z.object({
  nodes: z.array(
    z.object({
      id,
      kind: z.string(),
      label: z.string(),
      detail: z.record(z.string(), z.unknown()),
    }),
  ),
  edges: z.array(
    z.object({
      id,
      source: id,
      target: id,
      relationship: z.string(),
      detail: z.record(z.string(), z.unknown()),
    }),
  ),
  hidden_nodes: z.number(),
  snapshot: z.string(),
});
export type LineageData = z.infer<typeof lineageSchema>;
export const messageLineageSchema = lineageSchema.extend({
  root: id,
  readings: z.array(z.object({
    id, evidence_id: id, revision: id, created_at: z.string(), model: z.string(),
    processing_mode: z.string(), prior_interpretation_id: id.nullish(), reference_edition: z.string().nullish(),
    headline: z.string(), language: z.string().nullish(), location_label: z.string().nullish(),
    locations: z.number().int(), claims: z.number().int(), events: z.number().int(),
  })),
});
export type MessageLineageData = z.infer<typeof messageLineageSchema>;
export const importSchema = z.object({
  publication_id: id,
  accepted: z.number(),
  reused: z.number(),
  warnings: z.array(z.string()),
});
export const previewSchema = z.object({
  matched: z.number(),
  groups: z.number(),
  frequency: z.literal("unknown"),
  coverage: z.array(z.string()),
  baseline_policy: z.literal("fresh"),
  estimated_cost: z.string(),
});
export const backupSchema = z.object({
  id,
  created_at: z.string(),
  objects: z.number(),
  verified: z.boolean(),
  detail: z.string(),
});
export const verifySchema = z.object({
  verified: z.boolean(),
  objects: z.number(),
  briefs: z.number(),
  detail: z.string(),
});

export class ApiError extends Error {
  code: string;
  status: number;
  constructor(message: string, code: string, status: number) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.status = status;
  }
}
export async function api<T>(
  path: string,
  schema: z.ZodType<T>,
  options: { method?: string; body?: unknown; signal?: AbortSignal } = {},
): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    method: options.method ?? "GET",
    credentials: "same-origin",
    headers:
      options.body === undefined
        ? undefined
        : { "Content-Type": "application/json" },
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
    signal: options.signal,
  });
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    throw new ApiError(
      "The server returned an unreadable response. Your saved work has not been replaced.",
      "invalid_json",
      response.status,
    );
  }
  if (!response.ok) {
    const error = z
      .object({ error: z.object({ code: z.string(), message: z.string() }) })
      .safeParse(value);
    throw new ApiError(
      error.success
        ? error.data.error.message
        : "The request could not be completed.",
      error.success ? error.data.error.code : "request_failed",
      response.status,
    );
  }
  const parsed = schema.safeParse(value);
  if (!parsed.success)
    throw new ApiError(
      `The server response does not match API v1 (${path.split("?")[0]}: ${parsed.error.issues[0]?.path.join(".") || "root"}). Previous saved work remains available.`,
      "contract_mismatch",
      response.status,
    );
  return parsed.data;
}

export const label = (value: string) => value.replaceAll("_", " ");
export function evidencePreview(evidence: Pick<Evidence, "text" | "upstream_state" | "media">) {
  if (evidence.text) return evidence.text;
  if (evidence.upstream_state === "removed") return "Removed message";
  return evidence.media?.length ? "Media-only message" : "No retained text";
}
export function date(value: string | null | undefined, short = false) {
  if (!value) return "Not provided";
  return (
    new Intl.DateTimeFormat("en-GB", {
      day: "2-digit",
      month: "short",
      ...(short ? {} : { year: "numeric" }),
      hour: "2-digit",
      minute: "2-digit",
      timeZone: "UTC",
    }).format(new Date(value)) + " UTC"
  );
}
export function citationKey(c: Citation) {
  return `${c.evidence_id}@${c.revision}:${c.start_character}–${c.end_character}`;
}
