import type { InfiniteData } from "@tanstack/react-query";
import i18n from "../i18n";
import { call } from "../ipc/commands";
import { errorMessage } from "../ipc/errors";
import type { Label, MoveUndo, SplitRule, ThreadPage, ThreadSummary } from "../ipc/types";
import { isViewTab, labelTab, tabRoute, threadAfterMove, type TabRoute } from "../lib/tabDrop";
import { useUi } from "../stores/ui";
import { findCachedSummary } from "./actions";
import { queryClient } from "./client";

function refetchTabs() {
  // `splits`: a drop onto a label tab learns the sender into its condition.
  for (const key of ["threads", "bundles", "unreadCounts", "search", "thread", "splits"]) {
    void queryClient.invalidateQueries({ queryKey: [key] });
  }
}

/** The inbox tab a cached list holds, from its query key (see `threadsKey`
 *  and `useInboxBundles`), plus its account filter. Null for anything that
 *  isn't exactly one tab (other views, the unsplit inbox, user-label and
 *  folder filters). */
function cachedTab(
  key: readonly unknown[],
  autoLabels: ReadonlySet<number>,
): { tab: TabRoute; accountId: number | null } | null {
  if (key[0] === "bundles") {
    const tab = tabRoute({ splitId: key[1] as number | null, labelId: key[2] as number | null });
    return tab && { tab, accountId: key[3] as number | null };
  }
  const [, view, splitId, accountId, labelId, folderId] = key;
  if (view !== "inbox" || folderId != null) return null;
  if (labelId != null && !autoLabels.has(labelId as number)) return null;
  const tab = tabRoute({ splitId: splitId as number | null, labelId: labelId as number | null });
  return tab && { tab, accountId: accountId as number | null };
}

/** One cached page after the move: moved threads leave it (`leave`) or show
 *  their new labels and star in place, and `joining` threads are merged in
 *  newest first. */
function moveInPage(
  threads: ThreadSummary[],
  moved: ReadonlyMap<number, ThreadSummary>,
  leave: boolean,
  joining: ThreadSummary[] | null,
): ThreadSummary[] {
  const rest = leave
    ? threads.filter((t) => !moved.has(t.id))
    : threads.map((t) => moved.get(t.id) ?? t);
  if (!joining || joining.length === 0) return rest;
  const joined = new Set(joining.map((t) => t.id));
  return [...rest.filter((t) => !joined.has(t.id)), ...joining].sort(
    (a, b) => b.lastMessageAt - a.lastMessageAt,
  );
}

function applyOptimisticMove(ids: number[], to: TabRoute, from: TabRoute | null) {
  const splits = queryClient.getQueryData<SplitRule[]>(["splits"]);
  const labels = queryClient.getQueryData<Label[]>(["labels"]);
  const autoLabels = new Set((labels ?? []).filter((l) => l.isAuto).map((l) => l.id));
  const moved = new Map(
    ids.flatMap((id) => {
      const s = findCachedSummary(id);
      return s ? [[id, { ...s, ...threadAfterMove(s, to, from, splits, labels) }] as const] : [];
    }),
  );
  const joiningFor = (accountId: number | null) =>
    [...moved.values()].filter((s) => accountId == null || s.accountId === accountId);
  // A view tab target only adds; anything else takes the mail out of every
  // tab but view tabs. The label tab it was dragged out of loses it either way.
  const viewTarget = isViewTab(to, splits);
  const leaves = (tab: TabRoute) =>
    (tab === from && labelTab(from, splits) != null) || (!viewTarget && !isViewTab(tab, splits));

  for (const [key] of queryClient.getQueriesData({ queryKey: ["threads"] })) {
    const at = cachedTab(key, autoLabels);
    if (!at) continue;
    queryClient.setQueryData<InfiniteData<ThreadPage>>(key, (data) => {
      if (!data) return data;
      return {
        ...data,
        pages: data.pages.map((p, i) => ({
          ...p,
          threads: moveInPage(
            p.threads,
            moved,
            leaves(at.tab),
            at.tab === to && i === 0 ? joiningFor(at.accountId) : null,
          ),
        })),
      };
    });
  }
  for (const [key] of queryClient.getQueriesData({ queryKey: ["bundles"] })) {
    const at = cachedTab(key, autoLabels);
    if (!at) continue;
    queryClient.setQueryData<ThreadPage>(key, (page) =>
      page && {
        ...page,
        threads: moveInPage(
          page.threads,
          moved,
          leaves(at.tab),
          at.tab === to ? joiningFor(at.accountId) : null,
        ),
      },
    );
  }
}

/**
 * Drag-and-drop: move threads into inbox tab `to` (named `toName` in the
 * toast), dragged out of tab `from` when they all came from one. Optimistic
 * like archive: lists and bundles update at once, the backend plans and
 * applies the real change, then everything refetches. The toast offers Undo,
 * and Z undoes it too.
 */
export async function moveThreadsToTab(
  ids: number[],
  to: TabRoute,
  toName: string,
  from: TabRoute | null,
): Promise<void> {
  if (ids.length === 0) return;
  const ui = useUi.getState();
  const added = isViewTab(to, queryClient.getQueryData<SplitRule[]>(["splits"]));
  applyOptimisticMove(ids, to, from);
  if (ui.selection.length > 0) ui.clearSelection();
  try {
    const outcome = await call("move_threads_to_tab", { threadIds: ids, to, from });
    if (outcome.moved.length > 0) {
      const label = i18n.t(added ? "commands:actionLabel.addedToTab" : "commands:actionLabel.movedToTab", {
        name: toName,
      });
      const n = outcome.moved.length;
      useUi.getState().set({ lastUndo: { type: "move", undo: outcome.undo } });
      useUi.getState().pushToast({
        kind: "info",
        message: i18n.t("common:undoSuffix", { label: n > 1 ? `${label} (${n})` : label }),
        actionLabel: i18n.t("common:action.undo"),
        onAction: () => void undoMoveFromToast(outcome.undo),
      });
    }
    if (outcome.held.length > 0) {
      const splits = queryClient.getQueryData<SplitRule[]>(["splits"]);
      const holder = splits?.find((s) => s.id === outcome.held[0].splitId)?.name ?? "";
      useUi.getState().pushToast({
        kind: "info",
        message: i18n.t("inbox:drag.held", { count: outcome.held.length, name: holder }),
      });
    }
  } catch (err) {
    useUi.getState().pushToast({
      kind: "error",
      message: i18n.t("commands:actionFailed", { detail: errorMessage(err) }),
    });
  } finally {
    refetchTabs();
  }
}

/** Revert a move; true when the backend took it. */
export async function undoTabMove(undo: MoveUndo): Promise<boolean> {
  try {
    await call("undo_move_threads", { undo });
    return true;
  } catch {
    return false;
  } finally {
    refetchTabs();
  }
}

/** The toast's own Undo: reverts exactly its move, and stops Z from
 *  reverting it a second time. */
async function undoMoveFromToast(undo: MoveUndo) {
  const ui = useUi.getState();
  const last = ui.lastUndo;
  if (last?.type === "move" && last.undo === undo) ui.set({ lastUndo: null });
  const undone = await undoTabMove(undo);
  useUi.getState().pushToast({
    kind: undone ? "info" : "error",
    message: i18n.t(undone ? "commands:undo.undone" : "commands:undo.nothingToUndo"),
    durationMs: 2500,
  });
}
