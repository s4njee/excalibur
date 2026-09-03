import { describe, expect, it } from "vitest";
import { writeGuarded, type Backend } from "./backend";

/** A backend that records every call and never touches anything. */
function spyBackend() {
  const calls: string[] = [];
  const record = (name: string) => async () => {
    calls.push(name);
  };
  return {
    calls,
    backend: {
      mkdir: record("mkdir"),
      rename: record("rename"),
      remove: record("remove"),
      listDir: record("listDir"),
    } as unknown as Backend,
  };
}

describe("writeGuarded", () => {
  // Enumerated rather than spot-checked, so a write method added to `Backend`
  // and not guarded shows up as a missing case here.
  const writes: [string, (b: Backend) => Promise<void>][] = [
    ["mkdir", (b) => b.mkdir("new")],
    ["rename", (b) => b.rename("a", "b", false)],
    ["remove", (b) => b.remove("a", false)],
  ];

  for (const [name, call] of writes) {
    it(`does not send ${name} on a read-only mount`, async () => {
      const { calls, backend } = spyBackend();
      const guarded = writeGuarded(backend, () => false);
      await expect(call(guarded)).rejects.toBeDefined();
      // The point of the AC: not merely that it failed, but that nothing was
      // sent. A guard that let the call through and reported the engine's
      // refusal would pass a naive test and miss the requirement.
      expect(calls).toEqual([]);
    });

    it(`sends ${name} on a writable mount`, async () => {
      const { calls, backend } = spyBackend();
      const guarded = writeGuarded(backend, () => true);
      await call(guarded);
      expect(calls).toEqual([name]);
    });
  }

  it("re-reads writability per call rather than capturing it", async () => {
    // A mount can go read-only while the window is open, and a guard that
    // sampled the flag once at construction would keep sending writes.
    const { calls, backend } = spyBackend();
    let writable = true;
    const guarded = writeGuarded(backend, () => writable);
    await guarded.mkdir("first");
    writable = false;
    await expect(guarded.mkdir("second")).rejects.toBeDefined();
    expect(calls).toEqual(["mkdir"]);
  });

  it("leaves reads alone", async () => {
    const { calls, backend } = spyBackend();
    const guarded = writeGuarded(backend, () => false);
    await guarded.listDir("");
    expect(calls).toEqual(["listDir"]);
  });

  it("refuses with something the error reader understands", async () => {
    const { backend } = spyBackend();
    const guarded = writeGuarded(backend, () => false);
    const failure = await guarded.mkdir("x").catch((error: unknown) => error);
    const { engineFailure } = await import("./errors");
    expect(engineFailure(failure).message).toContain("read-only");
    expect(engineFailure(failure).surface).toBe("banner");
  });
});
