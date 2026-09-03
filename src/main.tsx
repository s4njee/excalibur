import { render } from "solid-js/web";
import App from "./App";
import "./tokens/tokens.css";

const root = document.getElementById("root");
if (!root) throw new Error("#root is missing from index.html");

// M0 renders the design's two browser screens from one component tree, chosen
// by query string, so `1a` and `1b` can be compared against the mock without a
// server. The real theme and mount come from preferences and the session once
// those land.
const parameters = new URLSearchParams(window.location.search);
const readOnly = parameters.get("screen") === "1b";
document.documentElement.dataset.theme = readOnly ? "dark" : "light";

render(() => <App readOnly={readOnly} />, root);
