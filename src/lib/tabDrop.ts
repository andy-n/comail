import type { Label, SplitRule } from "../ipc/types";
import { sameAnchor, tabMatch, type TabAnchor } from "./labelTabs";

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

const routeId = (route: TabRoute) => Number(route.slice(route.indexOf(":") + 1));

function splitOf(route: TabRoute | null, splits: SplitRule[] | undefined): SplitRule | undefined {
  if (!route?.startsWith("split:")) return undefined;
  const id = routeId(route);
  return (splits ?? []).find((s) => s.id === id);
}

/** The label tab (see `labelTabs.ts`) a route names: its split and what it
 *  shows. Null for any other tab. */
export function labelTab(
  route: TabRoute | null,
  splits: SplitRule[] | undefined,
): { rule: SplitRule; anchor: TabAnchor } | null {
  const rule = splitOf(route, splits);
  const anchor = rule ? tabMatch(rule) : null;
  return rule && anchor ? { rule, anchor } : null;
}

/** A view tab lists matching mail that stays where routing filed it. */
export function isViewTab(route: TabRoute | null, splits: SplitRule[] | undefined): boolean {
  return splitOf(route, splits)?.exclusive === false;
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
  if (route.startsWith("label:")) {
    const id = routeId(route);
    return (labels ?? []).some((l) => l.id === id && l.isAuto);
  }
  return labelTab(route, splits) != null;
}

/**
 * A thread's labels and star once it moved from tab `from` to `to`, for the
 * optimistic list update (the backend's `plan_move` is exact; this only has
 * to look right until the refetch). Leaving the Starred tab unstars and
 * leaving a view tab drops its label; the target's label or star is added.
 * A view tab target changes nothing else. Otherwise labels that file mail
 * into other exclusive label tabs go, and auto categories follow the route.
 */
export function threadAfterMove(
  thread: { labels: number[]; isStarred: boolean },
  to: TabRoute,
  from: TabRoute | null,
  splits: SplitRule[] | undefined,
  labels: Label[] | undefined,
): { labels: number[]; isStarred: boolean } {
  const target = labelTab(to, splits);
  const source = from === to ? null : labelTab(from, splits);
  let next = thread.labels;
  let isStarred = thread.isStarred;
  if (source && !(target && sameAnchor(target.anchor, source.anchor))) {
    const left = source.anchor;
    if (left.kind === "starred") isStarred = false;
    else if (source.rule.exclusive === false) next = next.filter((l) => l !== left.labelId);
  }
  const keep = target?.anchor.kind === "label" ? target.anchor.labelId : null;
  if (target?.rule.exclusive !== false) {
    const autoIds = new Set((labels ?? []).filter((l) => l.isAuto).map((l) => l.id));
    const tabLabels = new Set(
      (splits ?? []).flatMap((s) => {
        const a = s.exclusive === false ? null : tabMatch(s);
        return a?.kind === "label" ? [a.labelId] : [];
      }),
    );
    next = next.filter((l) => !autoIds.has(l) && (!tabLabels.has(l) || l === keep));
    if (to.startsWith("label:")) next = [...next, routeId(to)];
  }
  if (keep != null && !next.includes(keep)) next = [...next, keep];
  if (target?.anchor.kind === "starred") isStarred = true;
  return { labels: next, isStarred };
}
