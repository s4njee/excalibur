// What the window can honestly say about the machine it is running on.
//
// Only one caller so far — the title bar of a local mount, which has no
// hostname to put after the folder name because `file:` has no authority and
// the spec's `host` is empty (E21-S2). It is a separate file rather than a
// line in `App.tsx` so the rule is testable without a window, and so the next
// thing that needs it does not grow a second, differently-worded copy.

/**
 * The machine's name, from a user-agent string.
 *
 * "This Mac" is what macOS itself calls it — Finder's sidebar, the Get Info
 * panel, About This Mac — so on a Mac the window borrows the phrase the user
 * already reads everywhere else.
 *
 * Anywhere else it says "This computer", which is deliberately vaguer than the
 * hostname E21-S2's AC asks for on Linux. The webview has no way to ask the OS
 * its hostname: there is no Tauri command for it, and `navigator` knows the
 * platform but never the machine. "This computer" is true and adds nothing;
 * printing a guess would be a name the user could check and find wrong. A
 * `hostname` command is the fix, and is left for whoever needs the real name
 * (noted in the E21-S2 next steps).
 */
export function machineLabel(userAgent: string): string {
  return /macintosh|mac os/i.test(userAgent) ? "This Mac" : "This computer";
}

/** `machineLabel` for the machine this is actually running on. */
export function thisMachine(): string {
  return machineLabel(typeof navigator === "undefined" ? "" : navigator.userAgent);
}
