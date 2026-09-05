// The New Connection dialog's model: what each protocol needs, what a valid
// draft looks like, and the URI the dialog previews.
//
// Kept apart from the component so the rules are testable without rendering a
// dialog — and because "which fields does NFS have" is a fact about the
// protocol, not about a form.

import type { Access, Protocol } from "./types";

export interface Draft {
  protocol: Protocol;
  host: string;
  /** Share name, export path, or remote root, depending on the protocol. */
  share: string;
  port: string;
  username: string;
  password: string;
  domain: string;
  saveCredentials: boolean;
  mountPreference: Access;
}

export function emptyDraft(protocol: Protocol = "smb"): Draft {
  return {
    protocol,
    host: "",
    share: "",
    port: "",
    username: "",
    password: "",
    domain: "",
    saveCredentials: true,
    mountPreference: "rw",
  };
}

/** What the second field is called, which differs per protocol. */
export function shareLabel(protocol: Protocol): string {
  switch (protocol) {
    case "smb":
      return "Share";
    case "nfs":
      return "Export";
    case "sftp":
      return "Path";
    case "xsync":
      return "Export";
    case "local":
      return "Folder";
  }
}

/**
 * Whether the connection is to a server at all.
 *
 * Local is the first protocol with no other end. `file:` has no authority
 * component, the spec's `host` is `""`, and a Server field on that tab would
 * be a box that cannot be filled in usefully — so the dialog does not draw
 * one, the same way it does not draw NFS a password.
 */
export function usesHost(protocol: Protocol): boolean {
  return protocol !== "local";
}

/**
 * Whether there is an identity to choose.
 *
 * A folder on this machine is read and written as whoever is running the app.
 * There is no second account to offer, and a Username field would suggest the
 * mount could be made as somebody else.
 */
export function usesIdentity(protocol: Protocol): boolean {
  return protocol !== "local";
}

/**
 * The URI scheme, which is not always the protocol's name.
 *
 * Local's is `file`: it is the scheme every OS and every user already reads as
 * "a path on this machine", and inventing `local://` would be a URI nobody
 * else in the world writes.
 */
export function uriScheme(protocol: Protocol): string {
  return protocol === "local" ? "file" : protocol;
}

/** The example in the second field, which is a different shape per protocol. */
export function sharePlaceholder(protocol: Protocol): string {
  switch (protocol) {
    case "smb":
      return "media";
    case "local":
      // An absolute path, because that is the only thing `validate` accepts,
      // and `~` is not one: nothing here expands it.
      return "/Users/you/Documents";
    default:
      return "/srv/media";
  }
}

/** Whether this protocol authenticates with a username and password here. */
export function usesPassword(protocol: Protocol): boolean {
  // NFS has no password: its identity is the `sec=` mode plus, later, a
  // Kerberos principal. Showing an inert password field would suggest the
  // protocol works in a way it does not.
  return protocol === "smb" || protocol === "sftp";
}

/**
 * What the identity field is called, which is not "Username" everywhere.
 *
 * NFS has no login. AUTH_SYS sends a uid and gid the server chooses to
 * believe, so calling it a username would suggest a credential that does not
 * exist and an authentication step that never happens.
 */
export function usernameLabel(protocol: Protocol): string {
  return protocol === "nfs" ? "Identity" : "Username";
}

/**
 * Whether the identity belongs in the previewed URI.
 *
 * NFS's does not: `nfs://501@host/export` is not a form anyone writes, and the
 * uid is not a user component of the URI even though the form collects it in
 * the same place.
 */
export function identityInUri(protocol: Protocol): boolean {
  return protocol !== "nfs";
}

/** The hint under the identity field, where one helps. */
export function usernameHint(protocol: Protocol): string {
  return protocol === "nfs"
    ? "uid or uid:gid — defaults to yours. The server decides whether to believe it."
    : "";
}

/** SMB is the only one of the four with a Windows domain. */
export function usesDomain(protocol: Protocol): boolean {
  return protocol === "smb";
}

export function defaultPort(protocol: Protocol): number | null {
  switch (protocol) {
    case "sftp":
      return 22;
    case "nfs":
      return 2049;
    case "xsync":
      // xsync reaches the server by running `xs --server` over SSH; the daemon
      // with a port of its own is a later phase.
      return 22;
    default:
      return null;
  }
}

/**
 * The URI shown live under the form.
 *
 * Built from whatever has been typed so far, so it updates as the user types
 * rather than only once the form is valid. Empty fields simply do not appear.
 */
export function previewUri(draft: Draft): string {
  const scheme = uriScheme(draft.protocol);
  if (draft.protocol === "local") {
    // `file://` plus the path, verbatim. Nothing is percent-encoded: this is a
    // preview of the folder that will be opened, not a URL anything
    // dereferences, and a user checking that they picked the right directory
    // is better served by the path they typed than by `%20`.
    return `${scheme}://${draft.share || "…"}`;
  }
  const user = identityInUri(draft.protocol) && draft.username ? `${draft.username}@` : "";
  const host = draft.host || "…";
  const fallback = defaultPort(draft.protocol);
  const port = draft.port && Number(draft.port) !== fallback ? `:${draft.port}` : "";
  const share = draft.share ? `/${draft.share.replace(/^\/+/, "")}` : "";
  return `${scheme}://${user}${host}${share === "" ? "/" : share}${port === "" ? "" : port}`;
}

/** Why the draft cannot be connected yet, or `null` when it can. */
export function validate(draft: Draft): string | null {
  if (draft.protocol === "local") {
    const folder = draft.share.trim();
    if (!folder) return "Folder is required.";
    // Absolute, because a relative path has no meaning here: there is no
    // working directory the user can see, and resolving one against wherever
    // the app happened to be launched from would open a folder nobody chose.
    //
    // POSIX only, deliberately. plan.md's "Platforms (v1)" row packages macOS
    // and Linux; Windows is kept buildable but not shipped, so `C:\…` is not a
    // shape this has to accept yet — and accepting it untested would be worse
    // than refusing it.
    if (!folder.startsWith("/")) return "Folder must be an absolute path.";
    return null;
  }
  if (!draft.host.trim()) return "A server is required.";
  if (!draft.share.trim()) return `${shareLabel(draft.protocol)} is required.`;
  if (draft.port) {
    const port = Number(draft.port);
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      return "Port must be between 1 and 65535.";
    }
  }
  // A username without a password is normal — an agent or a key may answer —
  // so this deliberately does not require one.
  return null;
}

/** Whether the protocol has a backend behind it yet. */
export function isImplemented(protocol: Protocol): boolean {
  // SMB is answered with `Unsupported` by a build without its cargo feature.
  // Saying so in the dialog is better than letting a user fill in a form that
  // cannot connect.
  return protocol !== "smb";
}

/**
 * What the sidebar row and the window title call this connection.
 *
 * Shared by the spec and the bookmark so the two cannot drift. The last path
 * segment, because that is the name a person gave the thing; the host is the
 * fallback for a share with no path, and the raw share for a local root — `/`
 * has no last segment, and calling the whole disk "" would name nothing.
 */
export function connectionName(draft: Draft): string {
  return draft.share.split("/").filter(Boolean).pop() ?? (draft.host || draft.share || "/");
}

/**
 * The draft that a protocol tab produces from the one before it.
 *
 * The dialog swaps the field set rather than greying fields out (E3-S1), so
 * this is where "keep what still applies" is decided: a server and an account
 * name mean the same thing on every network protocol and survive the switch;
 * neither exists on Local, and carrying a hostname into a form with no Server
 * field would put it into the spec where nobody could see it. The share does
 * not survive: an SMB share name is not an NFS export and neither is a folder.
 */
export function switchProtocol(draft: Draft, protocol: Protocol): Draft {
  return {
    ...emptyDraft(protocol),
    host: usesHost(protocol) ? draft.host : "",
    username: usesIdentity(protocol) ? draft.username : "",
    mountPreference: draft.mountPreference,
  };
}

/**
 * The `ConnectionSpec` the engine takes.
 *
 * `mountPreference` is what the user asked for, and is all it is: the mount's
 * actual writability comes back from the server and may be less.
 */
export function toSpec(draft: Draft, id: string): Record<string, unknown> {
  return {
    id,
    name: connectionName(draft),
    protocol: draft.protocol,
    // Local's is `""`, and its `share` carries the absolute folder path: the
    // engine's `ConnectionSpec` keeps one shape for every protocol, and the
    // folder is the thing being opened, so it goes where the share goes.
    host: usesHost(draft.protocol) ? draft.host.trim() : "",
    port: draft.port ? Number(draft.port) : null,
    share: draft.share.trim(),
    // Kept for every protocol, NFS included: its identity is what the mount
    // will claim, so a bookmark that dropped it would reconnect as someone else.
    username: usesIdentity(draft.protocol) && draft.username ? draft.username : null,
    // Only SMB has one, and the dialog only shows the field there — but it was
    // being collected and then dropped, which is why an SMB share on a domain
    // could not be reached at all.
    domain: usesDomain(draft.protocol) && draft.domain ? draft.domain : null,
    mountPreference: draft.mountPreference,
  };
}
