import { updateSettings } from "../components/settings/SettingsPanel";
import type { Settings } from "../ipc/types";
import { activeInboxTabKey } from "../lib/splitOrder";
import type { TabPlacement } from "../lib/tabPlacement";
import { SPLIT_IMPORTANT, useUi } from "../stores/ui";
import { queryClient } from "./client";

/**
 * Move an inbox tab between a bundle in Important and a tab of its own in the
 * top bar (optimistic, like every settings write). Demoting the tab being
 * viewed returns to Important, where its bundle now lives. Shared by
 * Settings → Split inbox, the bundle row and the tab bar context menus.
 */
export function setTabPlacement(key: string, placement: TabPlacement): Promise<boolean> {
  const cur = queryClient.getQueryData<Settings>(["settings"])?.tabPlacement ?? {};
  const next = { ...cur };
  // Bundle is the default, so it is stored as a missing key.
  if (placement === "top") next[key] = "top";
  else delete next[key];
  const ui = useUi.getState();
  if (placement === "bundle" && activeInboxTabKey(ui.splitId, ui.labelFilter) === key) {
    ui.selectInboxTab({ splitId: SPLIT_IMPORTANT });
  }
  return updateSettings({ tabPlacement: next });
}

/** Mirror the backend after deleting a split (`s<id>`) or label (`l<id>`):
 *  it dropped the tab's placement, so drop it from the cached settings too
 *  (a refetch could race a pending settings write). */
export function forgetTabPlacement(key: string): void {
  queryClient.setQueryData<Settings>(["settings"], (s) => {
    if (!s?.tabPlacement || !(key in s.tabPlacement)) return s;
    const rest = { ...s.tabPlacement };
    delete rest[key];
    return { ...s, tabPlacement: rest };
  });
}
