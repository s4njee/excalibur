import { createRoot } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { INDICATOR_DELAY_MS, createListing } from "./listing";
import type { Listing } from "./types";

const mount: Listing["mount"] = {
  status: "ready",
  writable: true,
  reason: "",
  protocolLabel: "xsync v3",
  securityLabel: "ssh",
  options: "",
  capacity: null,
  caps: { randomAccess: true, resumeWrite: false, atomicRename: false, symlinks: true, watch: false },
};

const listingOf = (name: string): Listing => ({
  mount,
  entries: [
    {
      name,
      path: name,
      kind: "file",
      size: 1,
      modifiedMs: null,
      addedMs: null,
      mode: null,
      linkTarget: null,
      kindLabel: "Document",
      kindCategory: "other",
      hidden: false,
    },
  ],
});

/** A fetch whose promises are resolved by the test, in whatever order it likes. */
function deferred() {
  const pending: { path: string; resolve: (l: Listing) => void; reject: (e: unknown) => void }[] = [];
  const fetchListing = (path: string) =>
    new Promise<Listing>((resolve, reject) => pending.push({ path, resolve, reject }));
  /** The nth in-flight request. Throws rather than returning undefined, so a
      test that mis-counts fails on the line that mis-counted. */
  const nth = (index: number) => {
    const entry = pending[index];
    if (!entry) throw new Error(`no request ${index}; ${pending.length} in flight`);
    return entry;
  };
  return { pending, nth, fetchListing };
}

/** Lets the queued `.then`/`.finally` callbacks run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("createListing", () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
  afterEach(() => vi.useRealTimers());

  it("shows no indicator for a listing that arrives quickly", async () => {
    await createRoot(async (dispose) => {
      const { nth, fetchListing } = deferred();
      const state = createListing(fetchListing);

      state.load("a");
      expect(state.loading()).toBe(true);
      // A bar that appears and vanishes inside a fifth of a second reads as a
      // glitch, so nothing is shown yet.
      expect(state.slow()).toBe(false);

      vi.advanceTimersByTime(INDICATOR_DELAY_MS - 1);
      expect(state.slow()).toBe(false);

      nth(0).resolve(listingOf("quick"));
      await settle();

      expect(state.loading()).toBe(false);
      expect(state.slow()).toBe(false);
      expect(state.listing()?.entries[0]?.name).toBe("quick");
      dispose();
    });
  });

  it("raises the indicator once a listing passes the delay", async () => {
    await createRoot(async (dispose) => {
      const { nth, fetchListing } = deferred();
      const state = createListing(fetchListing);

      state.load("a");
      vi.advanceTimersByTime(INDICATOR_DELAY_MS);
      expect(state.slow()).toBe(true);

      nth(0).resolve(listingOf("slow"));
      await settle();
      expect(state.slow()).toBe(false);
      dispose();
    });
  });

  it("keeps the previous listing on screen while the next one loads", async () => {
    await createRoot(async (dispose) => {
      const { nth, fetchListing } = deferred();
      const state = createListing(fetchListing);

      state.load("a");
      nth(0).resolve(listingOf("first"));
      await settle();

      state.load("b");
      // Navigating into a large directory must not flash an empty window.
      expect(state.listing()?.entries[0]?.name).toBe("first");
      expect(state.loading()).toBe(true);
      dispose();
    });
  });

  it("ignores a reply for a path the user has already left", async () => {
    await createRoot(async (dispose) => {
      const { nth, fetchListing } = deferred();
      const state = createListing(fetchListing);

      state.load("slow-one");
      state.load("fast-one");

      // The second request answers first, then the first one finally lands.
      nth(1).resolve(listingOf("second"));
      await settle();
      nth(0).resolve(listingOf("first"));
      await settle();

      // The stale reply must not paint over where the user actually is.
      expect(state.listing()?.entries[0]?.name).toBe("second");
      expect(state.loading()).toBe(false);
      dispose();
    });
  });

  it("does not report an error from a superseded request", async () => {
    await createRoot(async (dispose) => {
      const { nth, fetchListing } = deferred();
      const state = createListing(fetchListing);

      state.load("doomed");
      state.load("fine");
      nth(0).reject(new Error("gone"));
      nth(1).resolve(listingOf("fine"));
      await settle();

      expect(state.error()).toBeNull();
      expect(state.listing()?.entries[0]?.name).toBe("fine");
      dispose();
    });
  });

  it("keeps the last good listing visible underneath an error", async () => {
    await createRoot(async (dispose) => {
      const { nth, fetchListing } = deferred();
      const state = createListing(fetchListing);

      state.load("a");
      nth(0).resolve(listingOf("good"));
      await settle();

      state.load("b");
      nth(1).reject(new Error("permission denied"));
      await settle();

      // A failed navigation that also wiped the view leaves nothing to go back
      // from.
      expect(state.error()).toBe("permission denied");
      expect(state.listing()?.entries[0]?.name).toBe("good");
      expect(state.loading()).toBe(false);
      dispose();
    });
  });
});
