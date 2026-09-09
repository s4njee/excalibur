// What the user typed into the path field (E1-S2).
//
// Three things can be typed there and they mean different things:
//
//   `Films/Arrival`      a path inside the share already open
//   `file:///Users/me`   a folder on this machine
//   `xsync://nas/media`  a share that may not be mounted yet
//
// Parsing is here, pure, because the interesting part is the ambiguity: a
// bare word is a path, `x://` is a URI, and something that looks like a URI
// but names a protocol this app has never heard of has to say so rather than
// being silently treated as a directory called `xyz:`.
//
// The URI shape is `previewUri`'s in `connection.ts`, read backwards. Those two
// are inverses and there is a round-trip test that says so; changing one
// without the other is how a URI the app itself printed stops being one the
// app can read.

import { emptyDraft, uriScheme, type Draft } from "./connection";
import type { Protocol } from "./types";

const PROTOCOLS: readonly Protocol[] = ["smb", "nfs", "sftp", "xsync", "local"];

/** Every scheme the field accepts, mapped to the protocol it names. */
const SCHEMES = new Map<string, Protocol>(
  PROTOCOLS.map((protocol) => [uriScheme(protocol), protocol]),
);

export type Location =
  /** A path inside the mount already open. */
  | { kind: "navigate"; path: string }
  /** A place that has to be mounted first. */
  | { kind: "connect"; draft: Draft }
  /** Typed something, but it cannot mean anything. */
  | { kind: "invalid"; reason: string };

/** `~` and `~/x`, against the home directory the window knows. */
function expandHome(path: string, home: string | null): string {
  if (home === null || path !== "~" && !path.startsWith("~/")) return path;
  return path === "~" ? home : `${home}/${path.slice(2)}`;
}

/**
 * Read a typed location.
 *
 * `null` for an empty field: nothing typed is not an error, it is a cancelled
 * edit, and the field should close rather than complain.
 */
export function parseLocation(input: string, home: string | null = null): Location | null {
  const text = input.trim();
  if (text === "") return null;

  const scheme = /^([A-Za-z][A-Za-z0-9+.-]*):\/\//.exec(text);
  if (scheme === null) {
    // No scheme: a path inside the share that is already open. Resolution
    // against the current directory is the caller's business — this only says
    // which of the three kinds it is.
    return { kind: "navigate", path: text };
  }

  const protocol = SCHEMES.get(scheme[1]!.toLowerCase());
  if (protocol === undefined) {
    return {
      kind: "invalid",
      reason: `${scheme[1]}:// is not a protocol this app speaks`,
    };
  }

  const rest = text.slice(scheme[0].length);

  if (protocol === "local") {
    // `file://` carries no authority here, so everything after it is the path
    // — `file:///Users/me` is `/Users/me`. A `localhost` authority is the one
    // spelling worth accepting, because it is what other tools print.
    const path = expandHome(rest.replace(/^localhost\//, "/"), home);
    if (!path.startsWith("/")) {
      return { kind: "invalid", reason: "a file:// location needs an absolute path" };
    }
    return { kind: "connect", draft: { ...emptyDraft("local"), share: path } };
  }

  // `[user@]host[:port]/share…`
  const authority = rest.split("/")[0] ?? "";
  const share = rest.slice(authority.length).replace(/^\/+/, "");
  const at = authority.lastIndexOf("@");
  const username = at === -1 ? "" : authority.slice(0, at);
  const hostPort = at === -1 ? authority : authority.slice(at + 1);
  const colon = hostPort.lastIndexOf(":");
  const host = colon === -1 ? hostPort : hostPort.slice(0, colon);
  const port = colon === -1 ? "" : hostPort.slice(colon + 1);

  if (host === "") {
    return { kind: "invalid", reason: `${scheme[1]}:// needs a host` };
  }
  if (port !== "" && !/^\d+$/.test(port)) {
    return { kind: "invalid", reason: `${port} is not a port` };
  }

  return {
    kind: "connect",
    draft: { ...emptyDraft(protocol), host, port, username, share },
  };
}
