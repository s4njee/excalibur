import { describe, expect, it } from "vitest";
import { machineLabel } from "./platform";

describe("the machine the window is running on", () => {
  it("calls a Mac what macOS calls it", () => {
    // Finder's sidebar, Get Info and About This Mac all say "This Mac", so the
    // title bar borrows the phrase rather than inventing a second one.
    expect(machineLabel("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15")).toBe(
      "This Mac",
    );
  });

  it("says something true rather than guessing a hostname elsewhere", () => {
    // The webview cannot ask the OS its hostname — there is no command for it,
    // and `navigator` knows the platform and never the machine. "This
    // computer" adds nothing and is at least not a name the user could check
    // and find wrong.
    expect(machineLabel("Mozilla/5.0 (X11; Linux x86_64)")).toBe("This computer");
    expect(machineLabel("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("This computer");
    expect(machineLabel("")).toBe("This computer");
  });
});
