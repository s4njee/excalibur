// Coalesced relisting (E8-S9).
//
// A directory gets relisted for several reasons — a mutation finished, a
// transfer landed, the user pressed ⌘R, and eventually a `watch` event from
// the server. Several of those arrive together: deleting twenty files produces
// twenty reasons to relist, and doing it twenty times is twenty round trips to
// show one result.
//
// The rule is the AC's: at most one relist per window, and an overflow — the
// server saying "too much changed to tell you what" — relists immediately,
// because the whole point of overflow is that nothing else can be trusted.

/** Injected so tests can drive time rather than wait for it. */
export interface Clock {
  now(): number;
  schedule(run: () => void, afterMs: number): number;
  cancel(handle: number): void;
}

export const realClock: Clock = {
  now: () => Date.now(),
  schedule: (run, afterMs) => setTimeout(run, afterMs) as unknown as number,
  cancel: (handle) => clearTimeout(handle),
};

export interface Coalescer {
  /** Something changed. Relists now, or at the end of the current window. */
  request(): void;
  /**
   * Too much changed to say what.
   *
   * Relists at once and resets the window: an overflow means the caller's
   * model of the directory is worthless, so making it wait would be showing
   * known-stale rows on purpose.
   */
  overflow(): void;
  /** Stop any pending relist. */
  dispose(): void;
}

/**
 * Wrap a relist so it runs at most once per `windowMs`.
 *
 * Leading *and* trailing: the first request runs immediately, so a single
 * change is not delayed a quarter second for nothing, and anything arriving
 * during the window runs once at its end, so the last state is never the one
 * left unshown.
 */
export function coalesce(
  relist: () => void,
  windowMs = 250,
  clock: Clock = realClock,
): Coalescer {
  let lastRun = Number.NEGATIVE_INFINITY;
  let pending: number | null = null;

  const run = () => {
    pending = null;
    lastRun = clock.now();
    relist();
  };

  return {
    request() {
      // Already waiting: that pending run will cover this request too. This is
      // the whole coalescing — twenty deletions share one relist.
      if (pending !== null) return;
      const since = clock.now() - lastRun;
      if (since >= windowMs) {
        run();
        return;
      }
      pending = clock.schedule(run, windowMs - since);
    },
    overflow() {
      if (pending !== null) {
        clock.cancel(pending);
        pending = null;
      }
      run();
    },
    dispose() {
      if (pending !== null) {
        clock.cancel(pending);
        pending = null;
      }
    },
  };
}
