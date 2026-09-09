import { describe, expect, it } from "vitest";
import {
  VERBOSE_ONLY,
  categoryLabel,
  formatLogTime,
  logText,
  resolutionNote,
  visibleLines,
} from "./log";
import type { LogLine } from "./types";

const line = (over: Partial<LogLine> = {}): LogLine => ({
  at: Date.parse("2026-08-31T09:22:41Z"),
  category: "command",
  text: "mount -t nfs4 -o sec=krb5p 10.0.0.4:/export/backups",
  resolution: "verb",
  ...over,
});

describe("what a quiet log leaves out", () => {
  it("keeps what was asked for, what happened and what to do about it", () => {
    const lines = [
      line({ category: "command" }),
      line({ category: "response" }),
      line({ category: "status" }),
      line({ category: "listing" }),
      line({ category: "hint" }),
      line({ category: "error" }),
    ];
    expect(visibleLines(lines, false).map((entry) => entry.category)).toEqual([
      "command",
      "status",
      "hint",
      "error",
    ]);
  });

  it("adds the chatty half when Verbose is on", () => {
    // A browse writes a response and a listing per directory, and neither is
    // the reason anybody opens a log.
    const lines = [line({ category: "response" }), line({ category: "listing" })];
    expect(visibleLines(lines, false)).toEqual([]);
    expect(visibleLines(lines, true)).toHaveLength(2);
    expect(VERBOSE_ONLY).toEqual(["response", "listing"]);
  });
});

describe("the log's strings", () => {
  it("labels a category the way the design writes it", () => {
    expect(categoryLabel("command")).toBe("Command:");
    expect(categoryLabel("error")).toBe("Error:");
  });

  it("draws a clock, not a date", () => {
    // A log is read about what just happened; a date would push the message
    // it is about off the end of the line.
    const at = new Date(2026, 7, 31, 9, 22, 41).getTime();
    expect(formatLogTime(at)).toBe("09:22:41");
  });

  it("pads every field so the lines align", () => {
    const at = new Date(2026, 7, 31, 9, 2, 4).getTime();
    expect(formatLogTime(at)).toBe("09:02:04");
  });

  it("has something to draw for a timestamp that is not one", () => {
    expect(formatLogTime(Number.NaN)).toBe("--:--:--");
  });

  it("copies what is on screen, as columns", () => {
    // Tab-separated so it lands in a bug report as columns rather than prose.
    const at = new Date(2026, 7, 31, 9, 22, 41).getTime();
    expect(logText([line({ at, category: "error", text: "EROFS" })])).toBe(
      "09:22:41\tError:\tEROFS",
    );
  });
});

describe("what the pane says it is showing", () => {
  it("does not imply frames it does not have", () => {
    // SMB, NFS and SFTP have no frames to expose; their mounts are recorded a
    // verb at a time, and a reader who believed otherwise would draw the wrong
    // conclusion from a log missing the frame they are looking for.
    expect(resolutionNote([line({ resolution: "verb" })])).toBe(
      "Engine operations — not protocol frames",
    );
  });

  it("says frames when that is what they are", () => {
    expect(resolutionNote([line({ resolution: "frame" })])).toBe("xsync protocol frames");
  });

  it("says both when a window holds both kinds of mount", () => {
    // One log, two resolutions: an xsync share and an SMB one open at once.
    expect(
      resolutionNote([line({ resolution: "frame" }), line({ resolution: "verb" })]),
    ).toBe("xsync protocol frames · engine operations elsewhere");
  });

  it("claims nothing for an empty log", () => {
    expect(resolutionNote([])).toBe("Engine operations — not protocol frames");
  });
});
