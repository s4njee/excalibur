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
  const scheme = draft.protocol;
  const user = identityInUri(draft.protocol) && draft.username ? `${draft.username}@` : "";
  const host = draft.host || "…";
  const fallback = defaultPort(draft.protocol);
  const port = draft.port && Number(draft.port) !== fallback ? `:${draft.port}` : "";
  const share = draft.share ? `/${draft.share.replace(/^\/+/, "")}` : "";
  return `${scheme}://${user}${host}${share === "" ? "/" : share}${port === "" ? "" : port}`;
}

/** Why the draft cannot be connected yet, or `null` when it can. */
export function validate(draft: Draft): string | null {
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
  // SMB and NFS are answered with `Unsupported` by the engine (E4, E5).
  // Saying so in the dialog is better than letting a user fill in a form that
  // cannot connect.
  return protocol === "xsync" || protocol === "sftp" || protocol === "nfs";
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
    name: draft.share.split("/").filter(Boolean).pop() ?? draft.host,
    protocol: draft.protocol,
    host: draft.host.trim(),
    port: draft.port ? Number(draft.port) : null,
    share: draft.share.trim(),
    // Kept for every protocol, NFS included: its identity is what the mount
    // will claim, so a bookmark that dropped it would reconnect as someone else.
    username: draft.username ? draft.username : null,
    // Only SMB has one, and the dialog only shows the field there — but it was
    // being collected and then dropped, which is why an SMB share on a domain
    // could not be reached at all.
    domain: usesDomain(draft.protocol) && draft.domain ? draft.domain : null,
    mountPreference: draft.mountPreference,
  };
}
