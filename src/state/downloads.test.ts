import { describe, expect, it } from "vitest";
import { DOWNLOAD_CATEGORIES, categorize, countByCategory, filterByCategory } from "./downloads";
import type { Transfer, TransferStatus } from "./types";

function row(id: string, status: TransferStatus): Transfer {
  return {
    id,
    direction: "down",
    from: { kind: "share", mount: "smb:nas.local:media", path: `remote/${id}` },
    to: { kind: "local", path: `/local/${id}` },
    name: id,
    sourcePath: `remote/${id}`,
    destPath: `/local/${id}`,
    bytesDone: 0,
    bytesTotal: 10,
    rate: null,
    etaSeconds: null,
    status,
    error: null,
    queuedAtMs: 1,
    mount: "smb:nas.local:media",
  };
}

describe("download categories", () => {
  it("groups queued and running as downloading", () => {
    expect(categorize("queued")).toBe("downloading");
    expect(categorize("running")).toBe("downloading");
    expect(categorize("paused")).toBe("paused");
    expect(categorize("done")).toBe("completed");
    expect(categorize("failed")).toBe("failed");
  });

  it("leaves cancelled rows to All alone", () => {
    // Neither finished work nor work awaiting an answer — only a record
    // until cleared.
    expect(categorize("cancelled")).toBeNull();
  });

  it("counts every category off the queue", () => {
    const transfers = [
      row("a", "queued"),
      row("b", "running"),
      row("c", "paused"),
      row("d", "done"),
      row("e", "failed"),
      row("f", "cancelled"),
    ];
    expect(countByCategory(transfers)).toEqual({
      all: 6,
      downloading: 2,
      paused: 1,
      completed: 1,
      failed: 1,
    });
  });

  it("filters to the chosen category, all means everything", () => {
    const transfers = [row("a", "paused"), row("b", "done")];
    expect(filterByCategory(transfers, "all").map((transfer) => transfer.id)).toEqual([
      "a",
      "b",
    ]);
    expect(filterByCategory(transfers, "paused").map((transfer) => transfer.id)).toEqual(["a"]);
    expect(filterByCategory(transfers, "failed")).toEqual([]);
  });

  it("lists the reference's five groups in its order", () => {
    expect(DOWNLOAD_CATEGORIES.map((category) => category.id)).toEqual([
      "all",
      "downloading",
      "paused",
      "completed",
      "failed",
    ]);
  });
});
