// Server-only data access over a local TinyCortex memory workspace.
//
// Everything here runs on the Node server (never shipped to the browser): it
// opens the workspace's SQLite stores read-only and reads the JSON/markdown
// artifacts a sync run leaves behind. Each reader tolerates a missing store so
// a partially-populated workspace still renders.

import "server-only";
import Database from "better-sqlite3";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

// ── Workspace resolution ────────────────────────────────────────────────────

export function workspacePath(): string | null {
  const raw = process.env.TINYCORTEX_WORKSPACE?.trim();
  return raw && raw.length > 0 ? raw : null;
}

export type StorePaths = {
  workspace: string;
  skillDocsDb: string;
  chunksDb: string;
  manifest: string;
  entitiesDir: string;
};

export function storePaths(workspace: string): StorePaths {
  return {
    workspace,
    skillDocsDb: join(workspace, "memory_tree", "skill_docs.db"),
    chunksDb: join(workspace, "memory_tree", "chunks.db"),
    manifest: join(workspace, "sync_manifest.json"),
    entitiesDir: join(workspace, "memory_tree", "content", "entities"),
  };
}

function openReadonly(path: string): Database.Database | null {
  if (!existsSync(path)) return null;
  try {
    return new Database(path, { readonly: true, fileMustExist: true });
  } catch {
    return null;
  }
}

// ── Skill documents (what sync ingested) ────────────────────────────────────

export type SkillDoc = {
  toolkit: string;
  documentId: string;
  namespaceSkillId: string;
  connectionId: string;
  title: string;
  content: string;
  metadata: Record<string, unknown>;
  updatedAt: number | null;
};

const SKILLDOC_NS_PREFIX = "skilldoc"; // ':' is sanitized to '_' by the KV store

function rowToDoc(namespace: string, valueJson: string, updatedAt: number | null): SkillDoc | null {
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(valueJson);
  } catch {
    return null;
  }
  const toolkit =
    (typeof parsed.toolkit === "string" && parsed.toolkit) ||
    namespace.slice(SKILLDOC_NS_PREFIX.length + 1) ||
    "unknown";
  return {
    toolkit,
    documentId: String(parsed.document_id ?? ""),
    namespaceSkillId: String(parsed.namespace_skill_id ?? toolkit),
    connectionId: String(parsed.connection_id ?? ""),
    title: String(parsed.title ?? "(untitled)"),
    content: String(parsed.content ?? ""),
    metadata:
      parsed.metadata && typeof parsed.metadata === "object"
        ? (parsed.metadata as Record<string, unknown>)
        : {},
    updatedAt,
  };
}

export function listSkillDocs(workspace: string): SkillDoc[] {
  const db = openReadonly(storePaths(workspace).skillDocsDb);
  if (!db) return [];
  try {
    const rows = db
      .prepare(
        "SELECT namespace, value_json, updated_at FROM kv_namespace ORDER BY updated_at DESC",
      )
      .all() as { namespace: string; value_json: string; updated_at: number }[];
    return rows
      .filter((r) => r.namespace.startsWith(SKILLDOC_NS_PREFIX))
      .map((r) => rowToDoc(r.namespace, r.value_json, r.updated_at))
      .filter((d): d is SkillDoc => d !== null);
  } finally {
    db.close();
  }
}

export function getSkillDoc(workspace: string, toolkit: string, documentId: string): SkillDoc | null {
  return (
    listSkillDocs(workspace).find(
      (d) => d.toolkit === toolkit && d.documentId === documentId,
    ) ?? null
  );
}

export type ToolkitCount = { toolkit: string; count: number };

export function skillDocCountsByToolkit(docs: SkillDoc[]): ToolkitCount[] {
  const counts = new Map<string, number>();
  for (const d of docs) counts.set(d.toolkit, (counts.get(d.toolkit) ?? 0) + 1);
  return [...counts.entries()]
    .map(([toolkit, count]) => ({ toolkit, count }))
    .sort((a, b) => b.count - a.count || a.toolkit.localeCompare(b.toolkit));
}

// ── Sync run manifest ───────────────────────────────────────────────────────

export type ToolkitResult = {
  toolkit: string;
  connectionId?: string;
  ingested?: number;
  actions?: number;
  costUsd?: number;
  docsStored?: number;
  taintOk?: boolean;
  cursorAdvanced?: boolean;
  idempotency?: string;
  passed?: boolean;
  error?: string | null;
};

export type SyncEvent = {
  toolkit?: string;
  sourceId?: string;
  source_id?: string;
  connectionId?: string;
  connection_id?: string;
  stage?: string;
  message?: string;
};

export type Manifest = {
  toolkits: ToolkitResult[];
  events: SyncEvent[];
  documentsPersisted?: number;
};

export function readManifest(workspace: string): Manifest | null {
  const path = storePaths(workspace).manifest;
  if (!existsSync(path)) return null;
  try {
    const parsed = JSON.parse(readFileSync(path, "utf8"));
    return {
      toolkits: Array.isArray(parsed.toolkits) ? parsed.toolkits : [],
      events: Array.isArray(parsed.events) ? parsed.events : [],
      documentsPersisted:
        typeof parsed.documentsPersisted === "number" ? parsed.documentsPersisted : undefined,
    };
  } catch {
    return null;
  }
}

// ── Memory tree (chunks) — present only after a full ingest ──────────────────

export type Chunk = {
  id: string;
  sourceKind: string | null;
  sourceId: string | null;
  pathScope: string | null;
  owner: string | null;
  tags: string[];
  preview: string;
  tokenCount: number | null;
  timestampMs: number | null;
};

export type ChunkView = { available: boolean; chunks: Chunk[]; total: number };

export function listChunks(workspace: string, limit = 200): ChunkView {
  const db = openReadonly(storePaths(workspace).chunksDb);
  if (!db) return { available: false, chunks: [], total: 0 };
  try {
    const total = (
      db.prepare("SELECT COUNT(*) AS n FROM mem_tree_chunks").get() as { n: number }
    ).n;
    const rows = db
      .prepare(
        `SELECT id, source_kind, source_id, path_scope, owner, tags_json,
                substr(content, 1, 240) AS preview, token_count, timestamp_ms
         FROM mem_tree_chunks
         ORDER BY created_at_ms DESC
         LIMIT ?`,
      )
      .all(limit) as Record<string, unknown>[];
    const chunks: Chunk[] = rows.map((r) => ({
      id: String(r.id),
      sourceKind: (r.source_kind as string) ?? null,
      sourceId: (r.source_id as string) ?? null,
      pathScope: (r.path_scope as string) ?? null,
      owner: (r.owner as string) ?? null,
      tags: parseTags(r.tags_json),
      preview: String(r.preview ?? ""),
      tokenCount: (r.token_count as number) ?? null,
      timestampMs: (r.timestamp_ms as number) ?? null,
    }));
    return { available: true, chunks, total };
  } catch {
    return { available: false, chunks: [], total: 0 };
  } finally {
    db.close();
  }
}

function parseTags(raw: unknown): string[] {
  if (typeof raw !== "string") return [];
  try {
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.map(String) : [];
  } catch {
    return [];
  }
}

// ── Entities — markdown files under memory_tree/content/entities ─────────────

export type Entity = { kind: string; file: string; frontMatter: Record<string, string>; body: string };

export function listEntities(workspace: string): Entity[] {
  const dir = storePaths(workspace).entitiesDir;
  if (!existsSync(dir)) return [];
  const out: Entity[] = [];
  let kinds: string[];
  try {
    kinds = readdirSync(dir, { withFileTypes: true })
      .filter((e) => e.isDirectory())
      .map((e) => e.name);
  } catch {
    return [];
  }
  for (const kind of kinds) {
    const kindDir = join(dir, kind);
    let files: string[];
    try {
      files = readdirSync(kindDir).filter((f) => f.endsWith(".md"));
    } catch {
      continue;
    }
    for (const file of files) {
      try {
        const { frontMatter, body } = parseFrontMatter(readFileSync(join(kindDir, file), "utf8"));
        out.push({ kind, file, frontMatter, body });
      } catch {
        // skip unreadable entity file
      }
    }
  }
  return out;
}

function parseFrontMatter(raw: string): { frontMatter: Record<string, string>; body: string } {
  const match = raw.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
  if (!match) return { frontMatter: {}, body: raw };
  const frontMatter: Record<string, string> = {};
  for (const line of match[1].split("\n")) {
    const idx = line.indexOf(":");
    if (idx > 0) frontMatter[line.slice(0, idx).trim()] = line.slice(idx + 1).trim();
  }
  return { frontMatter, body: match[2] };
}

// ── Overview aggregate ──────────────────────────────────────────────────────

export type Overview = {
  workspace: string;
  exists: { skillDocs: boolean; chunks: boolean; manifest: boolean; entities: boolean };
  docCount: number;
  toolkitCounts: ToolkitCount[];
  chunkTotal: number;
  entityCount: number;
  manifest: Manifest | null;
};

export function overview(workspace: string): Overview {
  const paths = storePaths(workspace);
  const docs = listSkillDocs(workspace);
  const chunks = listChunks(workspace, 1);
  const entities = listEntities(workspace);
  return {
    workspace,
    exists: {
      skillDocs: existsSync(paths.skillDocsDb),
      chunks: existsSync(paths.chunksDb),
      manifest: existsSync(paths.manifest),
      entities: existsSync(paths.entitiesDir),
    },
    docCount: docs.length,
    toolkitCounts: skillDocCountsByToolkit(docs),
    chunkTotal: chunks.total,
    entityCount: entities.length,
    manifest: readManifest(workspace),
  };
}
