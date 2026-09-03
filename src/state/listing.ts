// The listing on screen, and what it is doing.
//
// Two behaviours in the design make this more than `createResource`:
//
//   - **A slow listing must not blank the window.** The previous listing stays
//     on screen at half opacity while the next one loads, so navigating into a
//     large directory does not flash empty.
//   - **A fast listing must not flicker.** Under 200 ms there is no indicator
//     at all; showing and hiding a progress bar inside a fifth of a second
//     reads as a glitch rather than as progress.
//
// And navigation cancels: a reply for a path we have since left must never be
// painted. There is no `AbortController` reaching the engine — a Tauri command
// runs to completion regardless — so cancellation here means *ignoring* a
// stale reply. That is the honest word for it, and the generation counter is
// what enforces it.

import { createSignal, onCleanup, type Accessor } from "solid-js";
import type { Listing } from "./types";

/** How long a listing may take before the window admits it is waiting. */
export const INDICATOR_DELAY_MS = 200;

export interface ListingState {
  /** The most recent listing that arrived, kept while the next one loads. */
  listing: Accessor<Listing | null>;
  /** A request is in flight. */
  loading: Accessor<boolean>;
  /** In flight for longer than {@link INDICATOR_DELAY_MS}. Drives the bar. */
  slow: Accessor<boolean>;
  /** Why the last request failed, or null. */
  error: Accessor<string | null>;
  /** Load a path, superseding whatever was in flight. */
  load: (path: string) => void;
}

/**
 * Tracks one directory listing at a time.
 *
 * `load` is deliberately not async and returns nothing: every caller is an
 * event handler that has nothing useful to do with the promise, and returning
 * one invites an `await` that would be waiting on a request that may already
 * have been superseded.
 */
export function createListing(fetchListing: (path: string) => Promise<Listing>): ListingState {
  const [listing, setListing] = createSignal<Listing | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [slow, setSlow] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  // Incremented per request. A reply whose generation is not the current one
  // belongs to a path the user has already left.
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const stopTimer = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };

  onCleanup(stopTimer);

  const load = (path: string) => {
    const mine = ++generation;
    setLoading(true);
    setError(null);
    stopTimer();
    timer = setTimeout(() => {
      if (mine === generation) setSlow(true);
    }, INDICATOR_DELAY_MS);

    void fetchListing(path)
      .then((next) => {
        if (mine !== generation) return;
        setListing(next);
      })
      .catch((problem: unknown) => {
        if (mine !== generation) return;
        // The previous listing stays on screen underneath the error. A failed
        // navigation that also wiped the view would leave nothing to go back
        // from.
        setError(problem instanceof Error ? problem.message : String(problem));
      })
      .finally(() => {
        if (mine !== generation) return;
        stopTimer();
        setLoading(false);
        setSlow(false);
      });
  };

  return { listing, loading, slow, error, load };
}
