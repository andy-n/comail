import type { Settings } from "../ipc/types";
import type { InboxTabOrderItem } from "./splitOrder";

/** Where a non-Important inbox tab lives in the bundled avatar inbox: a bundle
 *  row inside Important, or a tab of its own in the tab bar. */
export type TabPlacement = "bundle" | "top";

/**
 * Tab-bar key of an inbox tab (`important`, `other`, `s<split id>`,
 * `l<auto label id>`); the same keys `activeInboxTabKey` returns and
 * `Settings.tabPlacement` is keyed by.
 */
export function inboxTabKey(item: InboxTabOrderItem): string {
  if (item.kind === "important") return "important";
  if (item.kind === "other") return "other";
  return item.kind === "split" ? `s${item.id}` : `l${item.id}`;
}

/** A tab's placement; anything but an explicit `top` is a bundle. Keys of
 *  deleted tabs are pruned by the backend when the tab is deleted. */
export function placementOf(
  placements: Settings["tabPlacement"] | undefined,
  key: string,
): TabPlacement {
  return placements?.[key] === "top" ? "top" : "bundle";
}

/** Whether the inbox folds other tabs into bundles (avatar UI with bundles
 *  on), the only mode where placement matters. */
export function isBundledInbox(
  settings: Pick<Settings, "inboxUi" | "showBundles"> | undefined,
): boolean {
  return settings?.inboxUi === "avatar" && settings.showBundles !== false;
}

/** Any inbox tab but Important. */
export type OtherInboxTab = Exclude<InboxTabOrderItem, { kind: "important" }>;

/** The tabs of the bundled inbox that stay bundles: every tab but Important
 *  and the ones moved to the top bar, in tab order. */
export function bundledTabOrder(
  order: InboxTabOrderItem[],
  placements: Settings["tabPlacement"] | undefined,
): OtherInboxTab[] {
  return order.filter(
    (item): item is OtherInboxTab =>
      item.kind !== "important" && placementOf(placements, inboxTabKey(item)) !== "top",
  );
}

/**
 * The tabs the tab bar lists, which Tab / Shift+Tab cycle and Cmd+1..9 jump
 * to. In the bundled inbox with at least one top tab that is Important plus
 * the top tabs, in tab order; the bundled tabs live in the Important list
 * instead. Otherwise every tab, as before placements existed.
 */
export function barTabOrder(
  order: InboxTabOrderItem[],
  placements: Settings["tabPlacement"] | undefined,
  bundledInbox: boolean,
): InboxTabOrderItem[] {
  if (!bundledInbox) return order;
  const top = order.filter(
    (item) => item.kind === "important" || placementOf(placements, inboxTabKey(item)) === "top",
  );
  return top.length > 1 ? top : order;
}

/** Whether the bundled inbox has any top tab, so the bar shows even where
 *  inline bundles would hide it. */
export function hasTopTabs(
  order: InboxTabOrderItem[],
  placements: Settings["tabPlacement"] | undefined,
  bundledInbox: boolean,
): boolean {
  return (
    bundledInbox &&
    order.some((item) => item.kind !== "important" && placementOf(placements, inboxTabKey(item)) === "top")
  );
}
