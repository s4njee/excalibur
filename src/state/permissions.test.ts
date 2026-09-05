import { describe, expect, it } from "vitest";
import {
  AFFORDANCES,
  READ_AFFORDANCES,
  RECOVERY_AFFORDANCES,
  WRITE_AFFORDANCES,
  acceptsDrop,
  gate,
  quickLookFooter,
  type Affordance,
  type Context,
} from "./permissions";

const readWrite: Context = { writable: true, reason: "", hasSelection: true };
const readOnly: Context = {
  writable: false,
  reason: "Read-only — export mapped `ro,root_squash`",
  hasSelection: true,
};

describe("the affordance list itself", () => {
  it("sorts every affordance into exactly one group", () => {
    // This is the property that makes the rest of the suite meaningful. A new
    // verb added to `Affordance` but to none of the groups fails here, so it
    // cannot slip through ungated by being forgotten.
    for (const affordance of AFFORDANCES) {
      const groups = [READ_AFFORDANCES, WRITE_AFFORDANCES, RECOVERY_AFFORDANCES].filter((group) =>
        group.includes(affordance),
      );
      expect(groups, `${affordance} is in ${groups.length} groups`).toHaveLength(1);
    }
  });

  it("has no duplicates", () => {
    expect(new Set(AFFORDANCES).size).toBe(AFFORDANCES.length);
  });
});

describe("every write affordance is gated on writable alone", () => {
  // The AC's property test: enumerated, not spot-checked, so a verb added
  // later is covered without anyone remembering to add a case.
  for (const affordance of WRITE_AFFORDANCES) {
    it(`disables ${affordance} on a read-only mount`, () => {
      expect(gate(affordance, readOnly).disabled).toBe(true);
    });

    it(`enables ${affordance} on a read-write mount`, () => {
      expect(gate(affordance, readWrite).disabled).toBe(false);
    });

    it(`explains ${affordance} with the server's own reason`, () => {
      // Verbatim: the server said why, and paraphrasing loses the detail that
      // tells a user whether it is their export or their credentials.
      expect(gate(affordance, readOnly).reason).toBe(readOnly.reason);
    });
  }
});

describe("read affordances", () => {
  for (const affordance of READ_AFFORDANCES) {
    it(`leaves ${affordance} available on a read-only mount`, () => {
      // Reading is exactly what a read-only mount is for.
      expect(gate(affordance, readOnly).disabled).toBe(false);
    });
  }
});

describe("recovery", () => {
  it("offers Remount as RW only when the mount is read-only", () => {
    expect(gate("remountRw", readOnly).disabled).toBe(false);
    expect(gate("remountRw", readWrite).disabled).toBe(true);
  });
});

describe("selection", () => {
  const nothingSelected = { ...readWrite, hasSelection: false };

  it("disables the verbs that need something to act on", () => {
    expect(gate("quickLook", nothingSelected).disabled).toBe(true);
    expect(gate("edit", nothingSelected).disabled).toBe(true);
  });

  it("does not gate Upload or New Folder on a selection", () => {
    // They act on the directory, not on rows.
    expect(gate("upload", nothingSelected).disabled).toBe(false);
    expect(gate("newFolder", nothingSelected).disabled).toBe(false);
  });

  it("prefers the read-only reason over 'nothing selected'", () => {
    // The one the user cannot fix by clicking a row is the more useful thing
    // to tell them.
    const neither: Context = { ...readOnly, hasSelection: false };
    expect(gate("edit", neither).reason).toBe(readOnly.reason);
  });
});

describe("derived surfaces", () => {
  it("refuses a drop on a read-only mount", () => {
    // Highlighting on drag-over and then refusing the drop is worse than
    // never highlighting.
    expect(acceptsDrop(readOnly)).toBe(false);
    expect(acceptsDrop(readWrite)).toBe(true);
  });

  it("tells the Quick Look footer what to do, not only what is refused", () => {
    expect(quickLookFooter(readOnly)).toBe("Read-only — download a copy to edit");
    expect(quickLookFooter(readWrite)).not.toContain("Read-only");
  });
});

describe("writable is the only input", () => {
  it("ignores everything else about the mount", () => {
    // Two mounts differing in every field except `writable` gate identically.
    const base = { hasSelection: true };
    const a: Context = { ...base, writable: false, reason: "one" };
    const b: Context = { ...base, writable: false, reason: "two" };
    for (const affordance of AFFORDANCES) {
      expect(gate(affordance, a).disabled, affordance).toBe(gate(affordance, b).disabled);
    }
  });

  it("refuses to edit something that is not text, and says so after the selection", () => {
    // Editing means handing the file to the user's `$EDITOR`. A 48 GB video
    // is not something to hand it (E14-S2).
    const writable = { writable: true, reason: "", hasSelection: true };
    expect(gate("edit", { ...writable, editable: false }).disabled).toBe(true);
    expect(gate("edit", { ...writable, editable: false }).reason).toMatch(/text/i);
    expect(gate("edit", { ...writable, editable: true }).disabled).toBe(false);
    // No opinion is not a refusal: a caller that does not know the type — a
    // test of the mount rule, say — is not gated on it.
    expect(gate("edit", writable).disabled).toBe(false);

    // And with nothing selected it is the selection that is missing, not the
    // type: complaining about the type of nothing helps nobody.
    const empty = { writable: true, reason: "", hasSelection: false, editable: false };
    expect(gate("edit", empty).reason).toBe("Nothing selected");
  });

  it("covers every affordance the type allows", () => {
    // A compile-time exhaustiveness check: if `Affordance` grows a member that
    // AFFORDANCES omits, this stops type-checking.
    const seen: Record<Affordance, true> = {
      quickLook: true,
      download: true,
      edit: true,
      upload: true,
      rename: true,
      delete: true,
      move: true,
      newFolder: true,
      dropTarget: true,
      remountRw: true,
      properties: true,
    };
    expect(Object.keys(seen).sort()).toEqual([...AFFORDANCES].sort());
  });
});
