import type { InfiniteData } from "@tanstack/react-query";
import i18n from "../i18n";
import { call } from "../ipc/commands";
import { errorMessage } from "../ipc/errors";
import type { Label, MoveUndo, SplitRule, ThreadPage, ThreadSummary } from "../ipc/types";
import { labelsAfterMove, tabRoute, type TabRoute } from "../lib/tabDrop";
import { useUi } from "../stores/ui";
import { findCachedSummary } from "./actions";
import { queryClient } from "./client";

function refetchTabs() {
  for (const key of ["threads", "bundles", "unreadCounts", "search", "thread"]) {
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

/** Moved threads leave every other tab's list and join the target's, newest
 *  first, so bundles and tabs update before the backend answers. */
function moveInPage(
  threads: ThreadSummary[],
  ids: ReadonlySet<number>,
  joining: ThreadSummary[] | null,
): ThreadSummary[] {
  const rest = threads.filter((t) => !ids.has(t.id));
  if (!joining || joining.length === 0) return rest;
  return [...rest, ...joining].sort((a, b) => b.lastMessageAt - a.lastMessageAt);
}

function applyOptimisticMove(ids: number[], to: TabRoute) {
  const splits = queryClient.getQueryData<SplitRule[]>(["splits"]);
  const labels = queryClient.getQueryData<Label[]>(["labels"]);
  const autoLabels = new Set((labels ?? []).filter((l) => l.isAuto).map((l) => l.id));
  const idSet = new Set(ids);
  const moved = ids.flatMap((id) => {
    const s = findCachedSummary(id);
    return s ? [{ ...s, labels: labelsAfterMove(s.labels, to, splits, labels) }] : [];
  });
  const joiningFor = (accountId: number | null) =>
    moved.filter((s) => accountId == null || s.accountId === accountId);

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
            idSet,
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
        threads: moveInPage(page.threads, idSet, at.tab === to ? joiningFor(at.accountId) : null),
      },
    );
  }
}

/**
 * Drag-and-drop: move threads into inbox tab `to` (named `toName` in the
 * toast). Optimistic like archive: lists and bundles update at once, the
 * backend plans and applies the real change, then everything refetches. The
 * toast offers Undo, and Z undoes it too.
 */
export async function moveThreadsToTab(ids: number[], to: TabRoute, toName: string): Promise<void> {
  if (ids.length === 0) return;
  const ui = useUi.getState();
  applyOptimisticMove(ids, to);
  if (ui.selection.length > 0) ui.clearSelection();
  try {
    const outcome = await call("move_threads_to_tab", { threadIds: ids, to });
    if (outcome.moved.length > 0) {
      const label = i18n.t("commands:actionLabel.movedToTab", { name: toName });
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
