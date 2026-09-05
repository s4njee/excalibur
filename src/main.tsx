import { render } from "solid-js/web";
import App from "./App";
import { fixtureBackend } from "./state/backend";
import "./tokens/tokens.css";

const root = document.getElementById("root");
if (!root) throw new Error("#root is missing from index.html");

// M0 renders the design's two browser screens from one component tree, chosen
// by query string, so `1a` and `1b` can be compared against the mock without a
// server. The real theme and mount come from preferences and the session once
// those land.
const parameters = new URLSearchParams(window.location.search);
const screen = parameters.get("screen");
const readOnly = screen === "1b";
document.documentElement.dataset.theme = readOnly ? "dark" : "light";

// `trust` and `keychanged` render E3-S4's two host-key states. Neither can be
// reached in a browser otherwise — there is no ssh-keyscan behind it — and both
// are states you cannot conjure on a real server just to look at them.
const hostKey =
  screen === "trust" ? "unknown" : screen === "keychanged" ? "changed" : undefined;

// `slow` renders E8-S1's loading state: a listing past the 200ms threshold,
// with the previous one still readable underneath. Both halves are invisible
// against a fixture that answers instantly.
const latencyMs = screen === "slow" ? 1500 : undefined;

// `huge` is E8-S2's 100k-row criterion, which needs a listing no fixture should
// carry. Nothing else about the window changes.
const rows = screen === "huge" ? 100_000 : undefined;

// `empty` renders E1-S6's first run: the desktop app with nothing mounted,
// which a browser cannot otherwise reach — the fixture always answers.
const disconnected = screen === "empty";

// `banner` renders E15-S1's connection failure: the strip under the toolbar,
// the dimmed list and the red health dot, all from one dropped mount event.
const dropped = screen === "banner";

/**
 * `transfers` renders E13's queue with one row of each state, which is the
 * only way to look at "failed with a Remount button" without breaking a share.
 */
const queue =
  screen === "transfers"
    ? [
        {
          id: "1", direction: "down" as const, name: "Arrival.2016.mkv",
          sourcePath: "Arrival.2016.mkv", destPath: "/Users/me/Downloads/Arrival.2016.mkv",
          bytesDone: 41_200_000_000, bytesTotal: 61_700_000_000,
          rate: 112_000_000, etaSeconds: 184, status: "running" as const,
          error: null, queuedAtMs: 1,
        },
        {
          id: "2", direction: "up" as const, name: "notes.txt",
          sourcePath: "/Users/me/notes.txt", destPath: "media/notes.txt",
          bytesDone: 0, bytesTotal: 4096, rate: null, etaSeconds: null,
          status: "queued" as const, error: null, queuedAtMs: 2,
        },
        {
          id: "3", direction: "down" as const, name: "cover-art.jpg",
          sourcePath: "cover-art.jpg", destPath: "/Users/me/Downloads/cover-art.jpg",
          bytesDone: 84_000, bytesTotal: 84_000, rate: null, etaSeconds: null,
          status: "done" as const, error: null, queuedAtMs: 3,
        },
        {
          id: "4", direction: "up" as const, name: "report.pdf",
          sourcePath: "/Users/me/report.pdf", destPath: "media/report.pdf",
          bytesDone: 0, bytesTotal: 120_000, rate: null, etaSeconds: null,
          status: "failed" as const,
          error: "read-only mount: export mapped `ro,root_squash`", queuedAtMs: 4,
        },
      ]
    : undefined;

render(
  () =>
    hostKey || latencyMs || rows || dropped || queue || disconnected ? (
      <App
        backend={fixtureBackend({ hostKey, latencyMs, rows, dropped, queue, disconnected })}
      />
    ) : (
      <App readOnly={readOnly} />
    ),
  root,
);
