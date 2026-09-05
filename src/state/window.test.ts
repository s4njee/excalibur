import { describe, expect, it } from "vitest";
import { FIRST_WINDOW, parseWindowParams } from "./window";

describe("window parameters", () => {
  it("reads the label, the mount and the path a new window was opened on", () => {
    // What `new_window` writes: a mount id is a bookmark's id, which carries
    // colons, and a path carries slashes and spaces. Both are percent-encoded
    // on the way out and decoded here.
    expect(
      parseWindowParams("?label=browser-2&path=Films%2FArrival%202016&mount=smb%3Anas.local%3Amedia"),
    ).toEqual({
      label: "browser-2",
      mount: "smb:nas.local:media",
      path: "Films/Arrival 2016",
    });
  });

  it("is the first window when there is no query at all", () => {
    // The config's window carries none: it is created before anything could
    // have put one there, and it starts with nothing mounted.
    expect(parseWindowParams("")).toEqual(FIRST_WINDOW);
    expect(parseWindowParams("?")).toEqual(FIRST_WINDOW);
  });

  it("leaves the design harness's own query alone", () => {
    // `?screen=1b` is how the browser preview picks a screen. It is not a
    // window parameter and must not become one.
    expect(parseWindowParams("?screen=1b")).toEqual(FIRST_WINDOW);
  });

  it("treats an empty mount as no mount rather than a share called nothing", () => {
    // The query is built by formatting, so a window opened with nothing
    // mounted can arrive with the key present and empty. `""` is not a
    // bookmark id, and a window that believed it was would draw the sidebar's
    // MOUNTED section over a share nobody has (E1-S6).
    expect(parseWindowParams("?label=browser-3&mount=&path=").mount).toBeNull();
  });

  it("keeps the share root distinguishable from no path", () => {
    // An empty path is the share root, which is a real place — not a missing
    // value. A window opened at the root must not fall back to anything else.
    expect(parseWindowParams("?label=browser-2&path=").path).toBe("");
  });
});
