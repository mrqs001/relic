// Run with Node >=22.18, against an already-running fictional workspace.
// Reads only; it does not import material or change analyst work.
import {
  workspaceSchema,
  querySchema,
  evidenceDetailSchema,
  lineageSchema,
  briefSchema,
  emptyScope,
} from "../src/api.ts";
const base = process.argv[2] || "http://127.0.0.1:8790";
const checks = [];
async function read(path, schema, body) {
  const response = await fetch(`${base}/api/v1${path}`, {
    method: body ? "POST" : "GET",
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!response.ok)
    throw new Error(
      `${path}: HTTP ${response.status}. This check needs a local-mode fictional validation workspace.`,
    );
  const parsed = schema.parse(await response.json());
  checks.push(path);
  return parsed;
}
const workspace = await read("/workspace", workspaceSchema);
if (!workspace.fictional || workspace.empty)
  throw new Error(
    "Use an explicitly imported fictional workspace. This script never imports automatically.",
  );
const result = await read("/query", querySchema, {
  scope: emptyScope,
  generation: 41,
  limit: 100,
});
if (
  result.generation !== 41 ||
  result.total !== result.located + result.unlocated
)
  throw new Error("Generation or shared population counts differ.");
if (
  result.items.length !== result.loaded ||
  result.map.features.length !== result.map_loaded
)
  throw new Error("Loaded subset counts differ.");
if (result.items[0]) {
  const selected = result.items[0];
  const exact = await read(
    `/evidence?id=${encodeURIComponent(selected.id)}&revision=${encodeURIComponent(selected.revision)}`,
    evidenceDetailSchema,
  );
  if (exact.id !== selected.id || exact.revision !== selected.revision)
    throw new Error("Evidence detail did not retain the requested identity.");
  await read(
    `/lineage?root=${encodeURIComponent(exact.lineage_root)}`,
    lineageSchema,
  );
}
if (workspace.briefs[0])
  await read(
    `/briefs/${encodeURIComponent(workspace.briefs[0].id)}`,
    briefSchema,
  );
const malformed = querySchema.safeParse({ ...result, generation: "41" });
if (malformed.success)
  throw new Error("Runtime decoder accepted an invalid generation type.");
console.log(
  JSON.stringify(
    {
      status: "passed",
      checks,
      generation: result.generation,
      eligible: result.total,
      located: result.located,
      unlocated: result.unlocated,
      malformed_generation_rejected: true,
    },
    null,
    2,
  ),
);
