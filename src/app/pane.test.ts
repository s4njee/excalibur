// One pane's state (XTP-E1-S1).
//
// The window held one of each of these as loose signals, which is why it could
// only ever show one directory. What matters here is that two panes are two
// panes: navigating, filtering or selecting in one must not reach the other.

import { createRoot } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { createPane } from "./pane";
import { EMPTY_SELECTION } from "../state/selection";
import type { Listing, MountInfo } from "../state/types";

const settle = () => new Promise((done) => setTimeout(done, 0));

/** A real `Listing`, not a cast: `mount` is required and the pane reads it. */
const MOUNT: MountInfo = {
  status: "ready",
  writable: true,
  reason: "",
  protocolLabel: "xsync v3",
  securityLabel: "ssh",
  options: "",
  capacity: null,
  caps: {
    randomAccess: true,
    resumeWrite: true,
    atomicRename: true,
    symlinks: true,
    watch: false,
  },
};
const emptyListing = async (): Promise<Listing> => ({ entries: [], mount: MOUNT });

/** Run a body inside a reactive owner, because the pane registers cleanups. */
function withPane<T>(run: (make: typeof createPane) => T): T {
  return createRoot((dispose) => {
    const result = run(createPane);
    dispose();
    return result;
  });
}

describe("a pane", () => {
  it("opens where it was told to", () => {
    withPane((make) => {
      expect(make({ path: "Films/Arrival", listDir: emptyListing }).path()).toBe(
        "Films/Arrival",
      );
      // A window opened on nothing starts at the share root.
      expect(make({ listDir: emptyListing }).path()).toBe("");
    });
  });

  it("moves, and goes back up", () => {
    withPane((make) => {
      const pane = make({ path: "Films", listDir: emptyListing });
      pane.goTo("Films/Arrival");
      expect(pane.path()).toBe("Films/Arrival");
      pane.ascend();
      expect(pane.path()).toBe("Films");
    });
  });

  it("is not two panes", () => {
    // The whole reason this exists. Two of them share no state at all.
    withPane((make) => {
      const left = make({ path: "Films", listDir: emptyListing });
      const right = make({ path: "Music", listDir: emptyListing });

      left.goTo("Films/Arrival");
      left.setFilter("mkv");
      left.setSelection({ paths: new Set(["Films/Arrival/a.mkv"]), anchor: null });

      expect(right.path()).toBe("Music");
      expect(right.filter()).toBe("");
      expect(right.selected().size).toBe(0);
      expect(left.selected().has("Films/Arrival/a.mkv")).toBe(true);
    });
  });

  it("lists through the reader it was given, not a shared one", async () => {
    const left = vi.fn(emptyListing);
    const right = vi.fn(emptyListing);
    await withPane(async (make) => {
      const a = make({ listDir: left });
      make({ listDir: right });
      a.listingState.load("Films");
      await settle();
      expect(left).toHaveBeenCalledWith("Films");
      expect(right).not.toHaveBeenCalled();
    });
  });

  it("starts empty unless it was handed a selection", () => {
    withPane((make) => {
      expect(make({ listDir: emptyListing }).selection()).toEqual(EMPTY_SELECTION);
      const seeded = make({
        listDir: emptyListing,
        selection: { paths: new Set(["a.mkv"]), anchor: "a.mkv" },
      });
      expect(seeded.selected().has("a.mkv")).toBe(true);
    });
  });

  it("says it is disconnected only once a request has failed", async () => {
    await withPane(async (make) => {
      const pane = make({
        listDir: async () => {
          throw new Error("no such mount");
        },
      });
      // Not before the first request settles: an empty state that flashed
      // before every window would be worse than none.
      expect(pane.disconnected()).toBe(false);
      pane.listingState.load("");
      await settle();
      await settle();
      expect(pane.disconnected()).toBe(true);
    });
  });

  it("coalesces the refreshes asked of it", async () => {
    const listDir = vi.fn(emptyListing);
    await withPane(async (make) => {
      const pane = make({ listDir });
      pane.refresh.request();
      pane.refresh.request();
      pane.refresh.request();
      await settle();
      // Twenty dropped files finishing is one relist, not twenty.
      expect(listDir.mock.calls.length).toBeLessThan(3);
    });
  });
});
