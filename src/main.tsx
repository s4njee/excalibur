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

render(
  () =>
    hostKey || latencyMs ? (
      <App backend={fixtureBackend({ hostKey, latencyMs })} />
    ) : (
      <App readOnly={readOnly} />
    ),
  root,
);
