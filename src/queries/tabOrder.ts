import i18n from "../i18n";
import { call } from "../ipc/commands";
import { errorMessage } from "../ipc/errors";
import type { Label, SplitRule } from "../ipc/types";
import { orderedTabKey, tabOrderPayload, type OrderedTab } from "../lib/splitOrder";
import { useUi } from "../stores/ui";
import { queryClient } from "./client";

/**
 * Persist a new shared tab order (custom splits + auto-label tabs, top to
 * bottom; see `mergeTabOrder`). Renumbers both caches first so the tab bar,
 * sidebar and Settings all reorder immediately, then reconciles with the
 * backend, which may re-route mail when the splits' priority changed.
 * Shared by Settings → Split inbox, the tab bar and the sidebar.
 */
export async function persistTabOrder(next: OrderedTab[]): Promise<void> {
  const posOf = new Map(next.map((it, i) => [orderedTabKey(it), i]));
  queryClient.setQueryData<SplitRule[]>(["splits"], (old) =>
    (old ?? []).map((s) => ({
      ...s,
      position: posOf.get(orderedTabKey({ kind: "split", id: s.id })) ?? s.position,
    })),
  );
  queryClient.setQueryData<Label[]>(["labels"], (old) =>
    (old ?? []).map((l) => ({
      ...l,
      position: posOf.get(orderedTabKey({ kind: "label", id: l.id })) ?? l.position,
    })),
  );
  try {
    await call("reorder_tabs", { order: tabOrderPayload(next) });
  } catch (err) {
    useUi.getState().pushToast({
      kind: "error",
      message: i18n.t("settings:splits.reorderFailed", { detail: errorMessage(err) }),
    });
  } finally {
    for (const key of ["splits", "labels", "threads", "bundles", "unreadCounts"]) {
      void queryClient.invalidateQueries({ queryKey: [key] });
    }
  }
}
