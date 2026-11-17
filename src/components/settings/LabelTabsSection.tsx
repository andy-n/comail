import { useState } from "react";
import { useTranslation } from "react-i18next";
import { call } from "../../ipc/commands";
import { errorMessage } from "../../ipc/errors";
import type { Label, SplitRule } from "../../ipc/types";
import { findTab, tabCondition, type TabAnchor } from "../../lib/labelTabs";
import { nextTabPosition } from "../../lib/splitOrder";
import { isBundledInbox } from "../../lib/tabPlacement";
import { useLabels, useSettings, useSplits } from "../../queries/hooks";
import { forgetTabPlacement } from "../../queries/tabPlacement";
import { SPLIT_IMPORTANT, useUi } from "../../stores/ui";
import { Icon } from "../ui/Icon";
import { SettingsGroup, Toggle } from "./panelKit";
import { invalidateSplitViews, TabPlacementControl } from "./SplitsPanel";

const anchorKey = (a: TabAnchor) => (a.kind === "starred" ? "starred" : `label:${a.labelId}`);

/**
 * Create or delete the tab showing `anchor`. A new tab is appended to the end
 * of the tab order and starts as a view tab (`hide` false): its mail stays in
 * Important too until the user hides it from the inbox.
 */
export async function setLabelTab(
  anchor: TabAnchor,
  name: string,
  on: boolean,
  splits: SplitRule[] | undefined,
  labels: Label[] | undefined,
  hide = false,
): Promise<void> {
  const existing = findTab(splits, anchor);
  if (on && !existing) {
    await call("save_split", {
      split: {
        id: null,
        name,
        position: nextTabPosition(splits, labels),
        query: {},
        condition: tabCondition({ anchor, include: [], exclude: [] }),
        target: null,
        ai: null,
        countAll: false,
        exclusive: hide,
      },
    });
  } else if (!on && existing) {
    await call("delete_split", { splitId: existing.id });
    forgetTabPlacement(`s${existing.id}`);
    // Don't leave the inbox pointed at a tab that no longer exists.
    if (useUi.getState().splitId === existing.id) {
      useUi.getState().set({
        splitId: SPLIT_IMPORTANT,
        selectedIndex: 0,
        selectedThreadId: null,
        selection: [],
      });
    }
  }
}

/** Switch an existing label tab between hiding its mail from the inbox and
 *  only listing it; the save re-routes the affected threads. */
async function setHidden(rule: SplitRule, hide: boolean): Promise<void> {
  await call("save_split", {
    split: {
      id: rule.id,
      name: rule.name,
      position: rule.position,
      query: rule.query,
      condition: rule.condition ?? null,
      target: rule.target ?? null,
      ai: rule.ai ?? null,
      countAll: rule.countAll ?? false,
      exclusive: hide,
    },
  });
}

/** Settings → Split inbox → Label tabs: one row per user label plus Starred,
 *  each with "Show as tab" and "Hide from inbox". */
export function LabelTabsSection() {
  const { t } = useTranslation();
  const pushToast = useUi((s) => s.pushToast);
  const { data: splits } = useSplits();
  const { data: labels } = useLabels();
  const { data: settings } = useSettings();
  // Bundle | Top tab, only where tabs fold into bundles.
  const bundledInbox = isBundledInbox(settings);
  const [busy, setBusy] = useState<string | null>(null);

  const rows: { anchor: TabAnchor; name: string; color?: string }[] = [
    { anchor: { kind: "starred" }, name: t("settings:splits.labelTabs.starred") },
    ...(labels ?? [])
      .filter((l) => !l.isAuto)
      .sort((a, b) => a.position - b.position)
      .map((l) => ({
        anchor: { kind: "label" as const, labelId: l.id },
        name: l.name,
        color: l.color,
      })),
  ];

  const run = async (anchor: TabAnchor, work: () => Promise<void>) => {
    if (busy != null) return;
    setBusy(anchorKey(anchor));
    try {
      await work();
    } catch (err) {
      pushToast({
        kind: "error",
        message: t("settings:splits.labelTabs.failed", { detail: errorMessage(err) }),
      });
    } finally {
      setBusy(null);
      invalidateSplitViews();
    }
  };

  return (
    <SettingsGroup
      title={t("settings:splits.labelTabs.title")}
      description={t("settings:splits.labelTabs.description")}
    >
      <div className="flex items-center gap-2.5 text-2xs font-semibold tracking-wide text-ink-faint uppercase">
        <span className="min-w-0 flex-1" />
        <span className="w-24 text-center">{t("settings:splits.labelTabs.showAsTab")}</span>
        <span className="w-24 text-center">{t("settings:splits.labelTabs.hideFromInbox")}</span>
        {bundledInbox && <span className="w-40 text-center">{t("settings:splits.placement.column")}</span>}
      </div>
      {rows.map(({ anchor, name, color }) => {
        const tab = findTab(splits, anchor);
        const hidden = tab != null && tab.exclusive !== false;
        const rowBusy = busy === anchorKey(anchor);
        return (
          <div key={anchorKey(anchor)} className="flex items-center gap-2.5">
            {anchor.kind === "starred" ? (
              <Icon name="star" size={13} filled className="shrink-0 text-star" />
            ) : (
              <span className="size-3 shrink-0 rounded-full" style={{ background: color }} />
            )}
            <span className="min-w-0 flex-1 truncate text-base text-ink">{name}</span>
            <span className="flex w-24 justify-center">
              <Toggle
                label={t("settings:splits.labelTabs.showAria", { name })}
                checked={tab != null}
                disabled={rowBusy}
                onChange={(on) =>
                  void run(anchor, () => setLabelTab(anchor, name, on, splits, labels))
                }
              />
            </span>
            <span className="flex w-24 justify-center">
              <Toggle
                label={t("settings:splits.labelTabs.hideAria", { name })}
                checked={hidden}
                disabled={tab == null || rowBusy}
                onChange={(on) => {
                  if (tab) void run(anchor, () => setHidden(tab, on));
                }}
              />
            </span>
            {bundledInbox && (
              <span className="flex w-40 justify-center">
                {tab && <TabPlacementControl tabKey={`s${tab.id}`} name={name} />}
              </span>
            )}
          </div>
        );
      })}
    </SettingsGroup>
  );
}
