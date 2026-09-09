import { describe, expect, it } from "vitest";
import { viewMenu, type ViewState } from "./viewMenu";

const view = (over: Partial<ViewState> = {}): ViewState => ({
  showHidden: false,
  foldersFirst: false,
  rowLayout: "columns",
  ...over,
});

const labelFor = (state: ViewState, action: string) =>
  viewMenu(state).find((item) => item.action === action)?.label ?? "";

describe("the View menu", () => {
  it("offers the toggles and every layout that exists", () => {
    expect(viewMenu(view()).map((item) => item.action)).toEqual([
      "toggleHidden",
      "toggleFoldersFirst",
      "layoutColumns",
      "layoutNameLed",
      "layoutMono",
      "layoutIcons",
      "layoutColumnView",
    ]);
  });

  it("ticks what is on", () => {
    expect(labelFor(view({ showHidden: true }), "toggleHidden")).toContain("✓");
    expect(labelFor(view({ showHidden: false }), "toggleHidden")).not.toContain("✓");
    expect(labelFor(view({ foldersFirst: true }), "toggleFoldersFirst")).toContain("✓");
  });

  it("ticks exactly one layout, because they are a choice not a set", () => {
    for (const layout of ["columns", "name-led", "mono", "icons", "column-view"] as const) {
      const ticked = viewMenu(view({ rowLayout: layout })).filter(
        (item) => item.action.startsWith("layout") && item.label.includes("✓"),
      );
      expect(ticked).toHaveLength(1);
    }
    expect(labelFor(view({ rowLayout: "name-led" }), "layoutNameLed")).toContain("✓");
  });

  it("names the state rather than the verb", () => {
    // "Hide Hidden Files" cannot be read without already knowing the answer.
    expect(labelFor(view(), "toggleHidden")).toContain("Show Hidden Files");
    expect(labelFor(view({ showHidden: true }), "toggleHidden")).toContain("Show Hidden Files");
  });

  it("keeps the labels aligned whether or not a row is ticked", () => {
    const on = labelFor(view({ showHidden: true }), "toggleHidden");
    const off = labelFor(view({ showHidden: false }), "toggleHidden");
    expect(on.length).toBe(off.length);
  });

  it("rules off the layouts from the toggles", () => {
    // They are different questions: two switches, then a choice of one.
    const items = viewMenu(view());
    expect(items.find((item) => item.action === "layoutColumns")?.separated).toBe(true);
  });
});
