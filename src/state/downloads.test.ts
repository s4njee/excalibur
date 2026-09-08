import { describe, expect, it } from "vitest";
import {
  DOWNLOAD_CATEGORIES,
  categorize,
  countByCategory,
  downloadUrlLines,
  filterByCategory,
  fractionOf,
  originHost,
  validDownloadDrop,
  validateDownloadUrl,
} from "./downloads";
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

describe("fractionOf", () => {
  it("is null while the size is unknown, like the engine's fraction", () => {
    // `Transfer::fraction` returns None when bytes_total is 0, and the two
    // must not disagree about what counts as knowing.
    expect(fractionOf({ bytesDone: 0, bytesTotal: 0 })).toBeNull();
    expect(fractionOf({ bytesDone: 512, bytesTotal: 0 })).toBeNull();
  });

  it("is a clamped ratio otherwise", () => {
    expect(fractionOf({ bytesDone: 25, bytesTotal: 100 })).toBe(0.25);
    expect(fractionOf({ bytesDone: 200, bytesTotal: 100 })).toBe(1);
    expect(fractionOf({ bytesDone: -5, bytesTotal: 100 })).toBe(0);
  });
});

describe("validateDownloadUrl", () => {
  it("accepts plain http and https URLs", () => {
    expect(validateDownloadUrl("https://example.test/big.iso")).toBeNull();
    expect(validateDownloadUrl("http://mirror.example:8080/a/b")).toBeNull();
    expect(validateDownloadUrl("  https://example.test/big.iso  ")).toBeNull();
  });

  it("refuses non-http schemes, spaces and control characters", () => {
    // The engine's `validate_http_url` refuses the same shape at enqueue
    // time; the dialog says so before Start instead.
    expect(validateDownloadUrl("ftp://example.test/f")).not.toBeNull();
    expect(validateDownloadUrl("example.test/f")).not.toBeNull();
    expect(validateDownloadUrl("https://example.test/my file.iso")).not.toBeNull();
    expect(validateDownloadUrl("https://example.test/a\tb")).not.toBeNull();
  });

  it("refuses credentialed authorities", () => {
    expect(validateDownloadUrl("https://user:pw@host/f")).not.toBeNull();
    expect(validateDownloadUrl("https://host")).toBeNull();
  });

  it("refuses query and fragment outright, the way the engine does", () => {
    expect(validateDownloadUrl("https://example.test/f?token=abc")).not.toBeNull();
    expect(validateDownloadUrl("https://example.test/f#part")).not.toBeNull();
  });
});

describe("download drops", () => {
  it("reads one URL per line and ignores URI-list comments", () => {
    expect(downloadUrlLines("# source\r\nhttps://a.test/one\n\nhttps://b.test/two\n")).toEqual([
      "https://a.test/one",
      "https://b.test/two",
    ]);
  });

  it("recognises only a wholly valid browser text drop", () => {
    expect(validDownloadDrop("https://a.test/one\nhttps://b.test/two")).toEqual([
      "https://a.test/one",
      "https://b.test/two",
    ]);
    expect(validDownloadDrop("some selected prose")).toEqual([]);
    expect(validDownloadDrop("https://a.test/one\nnot a URL")).toEqual([]);
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
