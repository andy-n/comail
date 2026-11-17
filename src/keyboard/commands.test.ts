import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { queryClient } from "../queries/client";
import { SPLIT_IMPORTANT, SPLIT_OTHER, useUi } from "../stores/ui";
import { ALL_COMMANDS } from "./commands";
import { buildCommandContext, inboxTabs } from "./context";

const preserved = {
  calendarDrawer: "week" as const,
  calendarFocusDay: 1746057600000,
  calendarView: "month" as const,
  sidebarPinned: true,
  accountFilter: 7,
};

beforeEach(() => {
  queryClient.clear();
  useUi.setState(useUi.getInitialState(), true);
  useUi.setState({
    ...preserved,
    calendarScreen: true,
    view: "sent",
    labelFilter: 90,
    folderFilter: 91,
    splitId: 92,
    searchOpen: true,
    searchQuery: "old query",
    openThreadId: 101,
    focusedMessageId: 102,
    selectedThreadId: 103,
    selectedIndex: 4,
    selection: [103, 104],
    selectAnchorId: 104,
  });
});

afterEach(() => {
  queryClient.clear();
  useUi.setState(useUi.getInitialState(), true);
});

async function runCommand(id: string) {
  const command = ALL_COMMANDS.find((candidate) => candidate.id === id);
  expect(command, id).toBeDefined();
  const context = buildCommandContext();
  expect(command!.when?.(context) ?? true).toBe(true);
  await command!.run(context);
}

function expectMailState(selectAnchorId: number | null = null) {
  expect(useUi.getState()).toMatchObject({
    ...preserved,
    calendarScreen: false,
    folderFilter: null,
    searchOpen: false,
    openThreadId: null,
    focusedMessageId: null,
    selectedThreadId: null,
    selectedIndex: 0,
    selection: [],
    selectAnchorId,
  });
}

describe("account switch commands", () => {
  it("reset the stale folder and selection when the scope changes", async () => {
    queryClient.setQueryData(["accounts"], [{ id: 3 }, { id: 8 }]);
    await runCommand("account-filter-1");
    expect(useUi.getState()).toMatchObject({
      accountFilter: 3,
      folderFilter: null,
      selection: [],
      selectAnchorId: null,
      selectedThreadId: null,
      selectedIndex: 0,
    });
  });

  it("leave the folder alone when re-picking the current account", async () => {
    queryClient.setQueryData(["accounts"], [{ id: 3 }, { id: 7 }]);
    await runCommand("account-filter-2");
    expect(useUi.getState()).toMatchObject({ accountFilter: 7, folderFilter: 91, selection: [103, 104] });
  });
});

describe("calendar to mail command navigation", () => {
  it.each([false, true])("cached label command (isAuto=%s) returns to mail", async (isAuto) => {
    // Slot zero must follow label position, not query-cache insertion order.
    queryClient.setQueryData(["labels"], [
      { id: 99, name: "Later", position: 10, isAuto: false },
      { id: 42, name: "Target", position: 0, isAuto },
    ]);
    await runCommand("go-label-0");
    // Auto-label navigation historically retains the range-selection anchor.
    expectMailState(isAuto ? 104 : null);
    expect(useUi.getState()).toMatchObject({
      view: isAuto ? "inbox" : "all",
      splitId: null,
      labelFilter: 42,
      searchQuery: isAuto ? "" : "old query",
    });
  });

  it.each([
    "inbox", "starred", "drafts", "sent", "done", "snoozed", "trash", "spam", "all",
  ] as const)("go-%s uses the real command context to leave calendar", async (view) => {
    await runCommand(`go-${view}`);
    expectMailState();
    expect(useUi.getState()).toMatchObject({
      view,
      splitId: view === "inbox" ? SPLIT_IMPORTANT : null,
      labelFilter: null,
      searchQuery: "old query",
    });
  });
});

describe("tab cycling in the bundled inbox", () => {
  beforeEach(() => {
    queryClient.setQueryData(["splits"], [
      { id: 1, name: "Starred", position: 0, query: {}, exclusive: false },
      { id: 2, name: "Newsletters", position: 1, query: {} },
    ]);
    queryClient.setQueryData(["labels"], [
      { id: 5, name: "Marketing", position: 2, color: "#000000", keyword: "ComailAutoMarketing", isAuto: true },
    ]);
    useUi.setState({
      calendarScreen: false,
      view: "inbox",
      splitId: SPLIT_IMPORTANT,
      labelFilter: null,
      folderFilter: null,
      searchOpen: false,
      openThreadId: null,
    });
  });

  const settings = (tabPlacement: Record<string, "bundle" | "top">, showBundles = true) =>
    queryClient.setQueryData(["settings"], { inboxUi: "avatar", showBundles, autoLabelsEnabled: true, tabPlacement });
  const tab = () => [useUi.getState().splitId, useUi.getState().labelFilter];

  it("Tab / Shift+Tab cycle Important and the top tabs only", () => {
    settings({ s1: "top", l5: "top" });
    const ctx = buildCommandContext();
    ctx.cycleSplit(1);
    expect(tab()).toEqual([1, null]);
    ctx.cycleSplit(1);
    expect(tab()).toEqual([null, 5]);
    ctx.cycleSplit(1);
    expect(tab()).toEqual([SPLIT_IMPORTANT, null]);
    ctx.cycleSplit(-1);
    expect(tab()).toEqual([null, 5]);
  });

  it("Cmd+N jumps among the bar's tabs", () => {
    settings({ l5: "top" });
    expect(inboxTabs().map((t) => [t.splitId, t.labelId])).toEqual([
      [SPLIT_IMPORTANT, null],
      [null, 5],
    ]);
    buildCommandContext().gotoSplitTab(1);
    expect(tab()).toEqual([null, 5]);
  });

  it("cycles every tab while nothing is on top or bundles are off", () => {
    const all = [
      [SPLIT_IMPORTANT, null],
      [1, null],
      [2, null],
      [null, 5],
      [SPLIT_OTHER, null],
    ];
    settings({});
    expect(inboxTabs().map((t) => [t.splitId, t.labelId])).toEqual(all);
    settings({ s1: "top" }, false);
    expect(inboxTabs().map((t) => [t.splitId, t.labelId])).toEqual(all);
  });
});
