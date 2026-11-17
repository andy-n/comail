import { describe, expect, it } from "vitest";
import type { Label, SplitRule } from "../ipc/types";
import { inboxTabOrder } from "./splitOrder";
import {
  barTabOrder,
  bundledTabOrder,
  hasTopTabs,
  inboxTabKey,
  isBundledInbox,
  placementOf,
} from "./tabPlacement";

const splits: SplitRule[] = [
  { id: 1, name: "Starred", position: 0, query: {}, exclusive: false },
  { id: 2, name: "Newsletters", position: 1, query: {} },
  // Routed into Other: owns no tab, so it never shows up anywhere.
  { id: 3, name: "Receipts", position: 3, query: {}, target: "other" },
];
const labels: Label[] = [
  { id: 5, name: "Marketing", position: 2, color: "#000000", keyword: "ComailAutoMarketing", isAuto: true },
];
const order = inboxTabOrder(splits, labels, true);
const keys = (items: Parameters<typeof inboxTabKey>[0][]) => items.map(inboxTabKey);

describe("tab placement", () => {
  it("defaults every tab to a bundle", () => {
    expect(placementOf(undefined, "s1")).toBe("bundle");
    expect(placementOf({ s1: "bundle" }, "s1")).toBe("bundle");
    expect(placementOf({ s1: "top" }, "s1")).toBe("top");
  });

  it("applies only to the avatar inbox with bundles on", () => {
    expect(isBundledInbox({ inboxUi: "avatar", showBundles: true })).toBe(true);
    expect(isBundledInbox({ inboxUi: "avatar", showBundles: false })).toBe(false);
    expect(isBundledInbox({ inboxUi: "classic", showBundles: true })).toBe(false);
    expect(isBundledInbox(undefined)).toBe(false);
  });

  it("bundles every other tab, in tab order, when nothing is on top", () => {
    expect(keys(bundledTabOrder(order, {}))).toEqual(["s1", "s2", "l5", "other"]);
    expect(keys(barTabOrder(order, {}, true))).toEqual(["important", "s1", "s2", "l5", "other"]);
    expect(hasTopTabs(order, {}, true)).toBe(false);
  });

  it("moves top tabs out of the bundles and into the bar after Important", () => {
    const placements = { s1: "top", l5: "top" } as const;
    expect(keys(bundledTabOrder(order, placements))).toEqual(["s2", "other"]);
    expect(keys(barTabOrder(order, placements, true))).toEqual(["important", "s1", "l5"]);
    expect(hasTopTabs(order, placements, true)).toBe(true);
  });

  it("can put Other on top too", () => {
    const placements = { other: "top" } as const;
    expect(keys(bundledTabOrder(order, placements))).toEqual(["s1", "s2", "l5"]);
    expect(keys(barTabOrder(order, placements, true))).toEqual(["important", "other"]);
  });

  it("ignores placements outside the bundled inbox and for unknown tabs", () => {
    const placements = { s1: "top", s99: "top" } as const;
    expect(keys(barTabOrder(order, placements, false))).toEqual(["important", "s1", "s2", "l5", "other"]);
    expect(hasTopTabs(order, placements, false)).toBe(false);
    expect(keys(barTabOrder(order, { s99: "top" }, true))).toEqual(["important", "s1", "s2", "l5", "other"]);
  });
});
