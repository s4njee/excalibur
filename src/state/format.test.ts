import { describe, expect, it } from "vitest";
import { extensionOf, formatCapacity, formatCounts, formatDuration, formatModified, formatModifiedGrid, formatMountLabels, formatSize, formatSizeTerse, formatTransferStatus } from "./format";

describe("the strings the design specifies", () => {
  it("renders the mock's own figures", () => {
    // These are the literal strings in the handoff, so a change to them is a
    // change to the design rather than a refactor.
    expect(formatSize(48_234_496_000)).toBe("48.2 GB");
    expect(formatCapacity(2_400_000_000_000, 16_000_000_000_000)).toBe("2.4 TB free of 16 TB");
    expect(formatCounts(13, 1, 48_234_496_000)).toBe("13 items · 1 selected · 48.2 GB");
  });

  it("omits the selection clause when nothing is selected", () => {
    expect(formatCounts(13, 0, 0)).toBe("13 items");
  });

  it("drops a trailing .0 so a round number reads as one", () => {
    // "2.4 TB free of 16 TB" appears in one sentence in the design, so the
    // decimal cannot be fixed at one place or at zero.
    expect(formatSize(16_000_000_000_000)).toBe("16 TB");
    expect(formatSize(2_400_000_000_000)).toBe("2.4 TB");
    // Not exactly 45, so it keeps the decimal rather than claiming to be.
    expect(formatSize(44_982_075_392)).toBe("45.0 GB");
  });

  it("keeps small sizes exact rather than rounding to zero", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(999)).toBe("999 B");
    expect(formatSize(6148)).toBe("6.1 KB");
  });

  it("drops the decimal once three digits are shown", () => {
    expect(formatSize(999_000_000_000)).toBe("999 GB");
  });

  it("formats a modification time the way the Modified column does", () => {
    // Constructed locally, because the column shows local time.
    const when = new Date(2026, 7, 31, 18, 24).getTime();
    expect(formatModified(when)).toBe("31 Aug 2026, 18:24");
    expect(formatModified(null)).toBe("—");
  });
});

describe("formatCounts with a filter", () => {
  it("says so when rows are hidden, rather than just showing a smaller number", () => {
    // "3 items" reads exactly like a directory containing three files, which
    // is the one thing a user must not conclude while a filter is on.
    expect(formatCounts(3, 0, 0, 13)).toBe("3 of 13 items");
  });

  it("stays plain when the filter matches everything", () => {
    expect(formatCounts(13, 0, 0, 13)).toBe("13 items");
  });

  it("keeps the selection parts alongside a filtered count", () => {
    expect(formatCounts(3, 1, 48_200_000_000, 13)).toBe("3 of 13 items · 1 selected · 48.2 GB");
  });

  it("is unchanged when no total is given", () => {
    expect(formatCounts(13, 0, 0)).toBe("13 items");
  });
});

describe("transfer row strings", () => {
  const running = {
    status: "running",
    bytesDone: 41_200_000_000,
    bytesTotal: 61_700_000_000,
    rate: 112_000_000,
    etaSeconds: 184,
  };

  it("produces the design's running string", () => {
    expect(formatTransferStatus(running)).toBe("41.2 / 61.7 GB · 112 MB/s · 3m 04s");
  });

  it("shows the unit once, on the total", () => {
    // "41.2 GB / 61.7 GB" is what a naive version produces and is not what
    // the design shows.
    expect(formatTransferStatus(running)).not.toContain("GB /");
  });

  it("leaves out what it cannot say rather than filling it in", () => {
    // A row reading "· — · —" is noise.
    expect(
      formatTransferStatus({ ...running, rate: null, etaSeconds: null }),
    ).toBe("41.2 / 61.7 GB");
  });

  it("says verified rather than done when the bytes were checked", () => {
    // Only the route through xsync's engine can set this, and the word is the
    // stronger claim: every byte was compared against a digest the source
    // computed. A chunk loop that hashed nothing must never reach it.
    expect(
      formatTransferStatus({ ...running, status: "done", bytesDone: 84_000, verified: true }),
    ).toBe("verified · 84 KB");
  });

  it("shows the note instead of the size on a copy that moved nothing", () => {
    // "done · unchanged · 84 KB" would read as though 84 KB moved.
    expect(
      formatTransferStatus({ ...running, status: "done", bytesDone: 84_000, note: "unchanged" }),
    ).toBe("done · unchanged");
  });

  it("says why a queued row is not moving, when there is a reason", () => {
    // A row waiting for the pool and a row whose share has never been
    // connected look identical, and at launch the second is the whole queue —
    // the engine leaves such a row queued rather than failing it and writes
    // the reason onto it, and this is the only place the user reads it.
    expect(
      formatTransferStatus({
        ...running,
        status: "queued",
        note: "waiting for its share",
      }),
    ).toBe("queued · waiting for its share");
  });

  it("says queued and done exactly as the design does", () => {
    expect(formatTransferStatus({ ...running, status: "queued" })).toBe("queued");
    expect(
      formatTransferStatus({ ...running, status: "done", bytesDone: 84_000 }),
    ).toBe("done · 84 KB");
  });

  it("falls back to a plain count when the size is unknown", () => {
    expect(
      formatTransferStatus({ ...running, bytesTotal: 0, rate: null, etaSeconds: null }),
    ).toBe("41.2 GB");
  });

  it("says how far through a folder it is, because one row is the whole tree", () => {
    // E22-S3: a folder is one row, so the bytes alone cannot say whether the
    // copy is on its tenth file or its ten-thousandth. The count is the
    // planner's, which is why it can be absent for the first moment.
    expect(
      formatTransferStatus({ ...running, filesDone: 412, filesTotal: 1002 }),
    ).toBe("41.2 / 61.7 GB · 412 / 1002 files · 112 MB/s · 3m 04s");
    expect(
      formatTransferStatus({
        ...running,
        status: "done",
        bytesDone: 84_000,
        filesDone: 1002,
        filesTotal: 1002,
      }),
    ).toBe("done · 1002 files · 84 KB");
  });

  it("says nothing about a count on a row that names one file", () => {
    // A row titled `e01.txt` that then announced `1 file` would be repeating
    // itself, and a row with no count at all has nothing to say yet.
    expect(formatTransferStatus({ ...running, filesDone: 1, filesTotal: 1 })).toBe(
      "41.2 / 61.7 GB · 112 MB/s · 3m 04s",
    );
    expect(
      formatTransferStatus({ ...running, status: "done", bytesDone: 84_000, filesTotal: 1 }),
    ).toBe("done · 84 KB");
  });
});

describe("formatDuration", () => {
  it("uses at most two units", () => {
    expect(formatDuration(12)).toBe("12s");
    expect(formatDuration(184)).toBe("3m 04s");
    expect(formatDuration(3720)).toBe("1h 02m");
  });

  it("pads the second field, so a column of them lines up", () => {
    expect(formatDuration(65)).toBe("1m 05s");
  });

  it("does not report a negative remaining time", () => {
    expect(formatDuration(-5)).toBe("0s");
  });
});

describe("the mono grid's formats", () => {
  it("abbreviates sizes to one unit character", () => {
    // `1h` aligns by character, so `48.2 GB` would push the next column out.
    expect(formatSizeTerse(48_200_000_000)).toBe("48.2G");
    expect(formatSizeTerse(2_000)).toBe("2.0K");
    expect(formatSizeTerse(812)).toBe("812");
    expect(formatSizeTerse(0)).toBe("0");
  });

  it("keeps the abbreviated size to five characters at most", () => {
    // The column's width is the point; a six-character size breaks the grid.
    for (const bytes of [999, 1_000, 99_900, 999_000, 1_000_000, 48_200_000_000, 9_000_000_000_000]) {
      expect(formatSizeTerse(bytes).length).toBeLessThanOrEqual(5);
    }
  });

  it("writes dates as MM-DD HH:MM, without the year", () => {
    const when = new Date(2026, 7, 31, 18, 24).getTime();
    expect(formatModifiedGrid(when)).toBe("08-31 18:24");
    expect(formatModifiedGrid(null)).toBe("—");
  });

  it("pads single-digit months and days so columns line up", () => {
    expect(formatModifiedGrid(new Date(2026, 0, 5, 9, 7).getTime())).toBe("01-05 09:07");
  });
});

describe("extensionOf", () => {
  it("takes the last extension, lowercased", () => {
    expect(extensionOf("Arrival.2016.mkv")).toBe("mkv");
    expect(extensionOf("COVER-ART.JPG")).toBe("jpg");
  });

  it("treats a leading dot as a hidden file, not an extension", () => {
    // `.DS_Store` is not a Store file.
    expect(extensionOf(".DS_Store")).toBe("—");
  });

  it("has no answer for a name without one", () => {
    expect(extensionOf("Documentaries")).toBe("—");
    expect(extensionOf("archive.")).toBe("—");
  });
});

describe("the status bar's protocol pair", () => {
  it("keeps two labels where they are two different facts", () => {
    expect(formatMountLabels("xsync v3", "ssh")).toBe("xsync v3 · ssh");
    expect(formatMountLabels("NFSv3", "sys")).toBe("NFSv3 · sys");
  });

  it("collapses them where they are the same word", () => {
    // A local folder is the case the design never had to consider: the engine
    // returns `local` for both, and `local · local` reads like a rendering bug
    // rather than an answer (E21-S2).
    expect(formatMountLabels("local", "local")).toBe("local");
  });
});
