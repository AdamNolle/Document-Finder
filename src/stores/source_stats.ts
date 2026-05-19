import { createStore } from "solid-js/store";
import type { DfEvent } from "@/lib/events";
import type { SourceId } from "@/lib/utils";

/// Lifetime per-source effectiveness stats. Persisted to localStorage so
/// the user gets a sense of "which sources actually deliver for me"
/// across runs and app launches.
///
/// Updated by listening to download_done / download_failed / source_done
/// events from the Tauri bridge (see main.tsx). Display is read by
/// SourcePanel as a small "1.2k saved · used in 12 runs" hint per row.
///
/// Storage shape: per source-id, counters for runs we've used this source
/// in, hits it returned during discovery, and downloads we actually saved.
/// `saved` is the most useful for "should I keep this enabled?" since it
/// reflects post-rank-and-extract success, not just raw hit count.

export interface SourceStat {
  saved: number;
  hits: number; // sum of source_done.count across runs
  runs: number; // distinct runs this source was queried in
  failed: number; // download_failed events attributed to this source
  lastUsedAt: number | null; // ms epoch
}

export type SourceStats = Record<string, SourceStat>;

const LS_KEY = "df-source-stats-v1";

function emptyStat(): SourceStat {
  return { saved: 0, hits: 0, runs: 0, failed: 0, lastUsedAt: null };
}

function loadSaved(): SourceStats {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return typeof parsed === "object" && parsed !== null ? (parsed as SourceStats) : {};
  } catch {
    return {};
  }
}

const [stats, setStats] = createStore<SourceStats>(loadSaved());

function persist() {
  try {
    localStorage.setItem(LS_KEY, JSON.stringify(stats));
  } catch {
    // Quota errors are very unlikely here (we store < 1 KB total), but
    // swallow rather than crash if they happen.
  }
}

/// Bucket a meta_search/<engine> source string into its underlying engine
/// id so per-engine stats reflect what users actually pick in the panel.
function normalize(src: string): string {
  if (src.startsWith("meta_search/")) return src.slice("meta_search/".length);
  return src;
}

function bump<K extends keyof SourceStat>(
  src: string,
  field: K,
  delta: SourceStat[K] extends number ? number : never,
) {
  const key = normalize(src);
  const existing = stats[key] ?? emptyStat();
  setStats(key, { ...existing, [field]: (existing[field] as number) + delta });
}

function touchLastUsed(src: string) {
  const key = normalize(src);
  const existing = stats[key] ?? emptyStat();
  setStats(key, { ...existing, lastUsedAt: Date.now() });
}

/// Hook into Tauri events. Counters update on:
///   - source_done  → hits +=count, runs +=1, lastUsedAt = now
///   - download_done   → saved +=1
///   - download_failed → failed +=1
/// Each call persists synchronously to localStorage so a crash doesn't
/// lose the most recent run's tally.
export function recordEvent(ev: DfEvent) {
  switch (ev.type) {
    case "source_done":
      bump(ev.payload.source, "hits", ev.payload.count);
      bump(ev.payload.source, "runs", 1);
      touchLastUsed(ev.payload.source);
      persist();
      break;
    case "download_done":
      bump(ev.payload.source, "saved", 1);
      touchLastUsed(ev.payload.source);
      persist();
      break;
    case "download_failed":
      bump(ev.payload.source, "failed", 1);
      persist();
      break;
  }
}

export const sourceStats = {
  /// Live snapshot of the stats store. Solid components reading
  /// individual fields will re-render automatically.
  get all() {
    return stats;
  },
  get(id: SourceId | string): SourceStat {
    return stats[normalize(id)] ?? emptyStat();
  },
  /// Wipe lifetime counters. Surfaces via Settings (handy after testing).
  clear() {
    for (const key of Object.keys(stats)) {
      setStats(key, emptyStat());
    }
    persist();
  },
};

/// Compact human-readable count. 1234 → "1.2k", 999 → "999".
export function formatCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(n / 1000).toFixed(1)}k`;
  return `${Math.round(n / 1000)}k`;
}
