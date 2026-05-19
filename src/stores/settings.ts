import { createStore } from "solid-js/store";
import { api } from "@/lib/tauri";
import { ALL_SOURCES, type SourceId } from "@/lib/utils";

const LS_KEY = "df-settings-v2";

function loadSaved(): Record<string, unknown> {
  try {
    return JSON.parse(localStorage.getItem(LS_KEY) ?? "{}");
  } catch {
    return {};
  }
}

const saved = loadSaved();

function posInt(v: unknown, fallback: number): number {
  return typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : fallback;
}
function safeStr(v: unknown, fallback: string): string {
  return typeof v === "string" && v.length > 0 ? v : fallback;
}
function safeSources(v: unknown): SourceId[] {
  if (!Array.isArray(v)) return [...ALL_SOURCES] as SourceId[];
  const valid = v.filter((s): s is SourceId => ALL_SOURCES.includes(s as SourceId));
  return valid.length > 0 ? valid : ([...ALL_SOURCES] as SourceId[]);
}
function safeUrl(v: unknown, fallback: string): string {
  if (typeof v !== "string") return fallback;
  if (v.trim() === "") return "";
  try { const u = new URL(v); if (u.protocol === "http:" || u.protocol === "https:") return v; } catch {}
  return fallback;
}

/// v2 defaulted searxngUrl to the Docker port http://localhost:8080. In v3
/// the embedded server is the source of truth and that URL is wrong, so
/// rewrite it to blank on load. Blank means "use the embedded server" on
/// both sides of the Tauri boundary.
function migrateSearxng(v: unknown): string {
  if (v === "http://localhost:8080") return "";
  return safeUrl(v, "");
}

export const [settings, setSettings] = createStore({
  libraryRoot:     safeStr(saved.libraryRoot, ""),
  perSource:       posInt(saved.perSource, 100),
  maxTotal:        posInt(saved.maxTotal, 500),
  concurrency:     posInt(saved.concurrency, 8),
  selectedSources: safeSources(saved.selectedSources),
  searxngUrl:      migrateSearxng(saved.searxngUrl),
});

if (!settings.libraryRoot) {
  api.defaultLibraryDir()
    .then(({ library_root }) => setSettings("libraryRoot", library_root))
    .catch(() => {});
}

export function saveSettings() {
  localStorage.setItem(LS_KEY, JSON.stringify(settings));
}

export function toggleSource(id: SourceId) {
  setSettings("selectedSources", (prev) =>
    prev.includes(id) ? prev.filter((s) => s !== id) : [...prev, id]
  );
  saveSettings();
}
