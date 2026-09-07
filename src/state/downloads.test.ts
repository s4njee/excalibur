import { describe, expect, it } from "vitest";
import { DOWNLOAD_CATEGORIES, categorize, countByCategory, filterByCategory, originHost } from "./downloads";
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

describe("origin hosts", () => {
  function hosted(id: string, overrides: Partial<Transfer> = {}): Transfer {
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
      status: "queued",
      error: null,
      queuedAtMs: 1,
      mount: "smb:nas.local:media",
      ...overrides,
    };
  }

  it("redacts credentials out of an HTTP origin", () => {
    // The engine refuses credentialed URLs at creation, but a restored row
    // carries whatever the file held — display must not trust it.
    expect(
      originHost(
        hosted("secret", {
          direction: null,
          mount: null,
          from: { kind: "http", url: "https://user:pw@host/f" },
        }),
      ),
    ).toBe("host");
  });

  it("takes the hostname, not the URL", () => {
    expect(
      originHost(
        hosted("f", { from: { kind: "http", url: "https://EXAMPLE.test:8443/big.iso" } }),
      ),
    ).toBe("example.test");
  });

  it("names the share's host and This Mac for local origins", () => {
    expect(originHost(hosted("a"))).toBe("nas.local");
    expect(
      originHost(hosted("b", { from: { kind: "local", path: "/local/b" } })),
    ).toBe("This Mac");
  });

  it("resolves legacy rows through direction and mount", () => {
    const legacy = hosted("c");
    delete legacy.from;
    delete legacy.to;
    expect(originHost(legacy)).toBe("nas.local");
    expect(originHost({ ...legacy, direction: "up" })).toBe("This Mac");
    expect(originHost({ ...legacy, direction: null, mount: null })).toBeNull();
  });

  it("calls an unmounted share a share rather than inventing a host", () => {
    expect(
      originHost(hosted("d", { from: { kind: "share", mount: null, path: "remote/d" } })),
    ).toBe("Share");
  });
});
