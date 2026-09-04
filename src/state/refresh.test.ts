import { describe, expect, it } from "vitest";
import { coalesce, type Clock } from "./refresh";

/** A clock the test moves by hand. */
function fakeClock() {
  let now = 1000;
  let next = 1;
  const timers = new Map<number, { at: number; run: () => void }>();
  const clock: Clock = {
    now: () => now,
    schedule(run, afterMs) {
      const handle = next++;
      timers.set(handle, { at: now + afterMs, run });
      return handle;
    },
    cancel(handle) {
      timers.delete(handle);
    },
  };
  return {
    clock,
    advance(ms: number) {
      now += ms;
      for (const [handle, timer] of [...timers]) {
        if (timer.at <= now) {
          timers.delete(handle);
          timer.run();
        }
      }
    },
    pending: () => timers.size,
  };
}

describe("coalesce", () => {
  it("runs the first request immediately", () => {
    // A single change should not wait a quarter second to be shown.
    const { clock } = fakeClock();
    let runs = 0;
    coalesce(() => runs++, 250, clock).request();
    expect(runs).toBe(1);
  });

  it("collapses a burst into one trailing relist", () => {
    // Deleting twenty files gives twenty reasons to relist and one result.
    const time = fakeClock();
    let runs = 0;
    const refresh = coalesce(() => runs++, 250, time.clock);

    refresh.request();
    expect(runs).toBe(1);
    for (let i = 0; i < 20; i++) refresh.request();
    expect(runs, "the burst must not relist per change").toBe(1);

    time.advance(250);
    expect(runs, "but the last state must still be shown").toBe(2);
  });

  it("does not leave the final change unshown", () => {
    // The trailing edge is the point: a leading-only throttle would drop the
    // last change of a burst and leave the list wrong until something else
    // happened to refresh it.
    const time = fakeClock();
    let runs = 0;
    const refresh = coalesce(() => runs++, 250, time.clock);
    refresh.request();
    time.advance(10);
    refresh.request();
    time.advance(240);
    expect(runs).toBe(2);
  });

  it("starts a fresh window rather than running twice in one", () => {
    const time = fakeClock();
    let runs = 0;
    const refresh = coalesce(() => runs++, 250, time.clock);
    refresh.request();
    time.advance(300);
    refresh.request();
    expect(runs).toBe(2);
    time.advance(300);
    expect(runs, "nothing was pending, so nothing more should run").toBe(2);
  });

  it("relists at once on overflow", () => {
    // Overflow means the server could not say what changed, so every row on
    // screen is suspect. Waiting out the window would be showing known-stale
    // rows deliberately.
    const time = fakeClock();
    let runs = 0;
    const refresh = coalesce(() => runs++, 250, time.clock);
    refresh.request();
    refresh.request();
    expect(runs).toBe(1);
    refresh.overflow();
    expect(runs).toBe(2);
  });

  it("cancels the pending relist an overflow supersedes", () => {
    const time = fakeClock();
    let runs = 0;
    const refresh = coalesce(() => runs++, 250, time.clock);
    refresh.request();
    refresh.request();
    refresh.overflow();
    time.advance(500);
    expect(runs, "the superseded relist must not also run").toBe(2);
    expect(time.pending()).toBe(0);
  });

  it("stops pending work when disposed", () => {
    // The directory is gone or the window closed; relisting it would be a
    // request for something nobody is looking at.
    const time = fakeClock();
    let runs = 0;
    const refresh = coalesce(() => runs++, 250, time.clock);
    refresh.request();
    refresh.request();
    refresh.dispose();
    time.advance(500);
    expect(runs).toBe(1);
    expect(time.pending()).toBe(0);
  });
});
