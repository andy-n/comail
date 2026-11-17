import type { Label, RuleCondition, SplitRule } from "../ipc/types";
import { queryToCondition } from "./ruleCondition";

/** An inbox tab as `move_threads_to_tab` addresses it (a route key). */
export type TabRoute = "important" | "other" | `split:${number}` | `label:${number}`;

// SPLIT_IMPORTANT / SPLIT_OTHER (stores/ui); literals keep this module store-free.
const IMPORTANT = -1;
const OTHER = -2;

/** Route key of an inbox tab given as a split id / auto-label id pair (tab
 *  bar items, bundles, list caches). Null for the unsplit inbox. */
export function tabRoute(tab: { splitId?: number | null; labelId?: number | null }): TabRoute | null {
  if (tab.labelId != null) return `label:${tab.labelId}`;
  if (tab.splitId === IMPORTANT) return "important";
  if (tab.splitId === OTHER) return "other";
  if (tab.splitId != null && tab.splitId > 0) return `split:${tab.splitId}`;
  return null;
}

/** Label ids of a label-only condition (a bare label, or any-of labels, also
 *  wrapped in a one-child `all` as legacy queries are); null for anything
 *  else. Mirrors `label_only` in route/manual.rs. */
function labelOnly(c: RuleCondition): number[] | null {
  if (c.kind === "all" && c.of.length === 1) return labelOnly(c.of[0]);
  if (c.kind === "label") return [c.id];
  if (c.kind !== "any" || c.of.length === 0) return null;
  const ids: number[] = [];
  for (const leaf of c.of) {
    if (leaf.kind !== "label") return null;
    ids.push(leaf.id);
  }
  return ids;
}

/** The labels that file mail into this split, when a drop can do it by hand:
 *  its own tab, no AI check, and a label-only condition. Mirrors `claim_of`. */
export function splitDropLabels(rule: SplitRule): number[] | null {
  if (rule.target != null || rule.ai) return null;
  return labelOnly(rule.condition ?? queryToCondition(rule.query));
}

/** Whether dragged mail can be dropped onto this tab. Mirrors `droppable` in
 *  route/manual.rs: label tabs, AI categories, Important and Other take mail;
 *  sender / subject rule tabs don't. */
export function canDropOnTab(
  route: TabRoute,
  splits: SplitRule[] | undefined,
  labels: Label[] | undefined,
): boolean {
  if (route === "important" || route === "other") return true;
  const id = Number(route.slice(route.indexOf(":") + 1));
  if (route.startsWith("label:")) return (labels ?? []).some((l) => l.id === id && l.isAuto);
  const rule = (splits ?? []).find((s) => s.id === id);
  return rule != null && splitDropLabels(rule) != null;
}

/**
 * A thread's label ids as they will look once it moved to `to`, for the
 * optimistic list update (the backend's plan is exact; this only has to look
 * right until the refetch). Labels that file mail into other label tabs go,
 * the target tab's label is added; auto categories follow the route.
 */
export function labelsAfterMove(
  current: number[],
  to: TabRoute,
  splits: SplitRule[] | undefined,
  labels: Label[] | undefined,
): number[] {
  const autoIds = new Set((labels ?? []).filter((l) => l.isAuto).map((l) => l.id));
  const tabLabels = new Set((splits ?? []).flatMap((s) => splitDropLabels(s) ?? []));
  const id = Number(to.slice(to.indexOf(":") + 1));
  const rule = to.startsWith("split:") ? (splits ?? []).find((s) => s.id === id) : undefined;
  const keep = (rule && splitDropLabels(rule)) ?? [];
  const next = current.filter((l) => !autoIds.has(l) && (!tabLabels.has(l) || keep.includes(l)));
  if (keep.length > 0 && !next.some((l) => keep.includes(l))) next.push(keep[0]);
  if (to.startsWith("label:")) next.push(id);
  return next;
}
