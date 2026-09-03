// Reading the engine's errors on the frontend side.
//
// A Tauri command rejects with the serialised `CommandError`, a plain object —
// not an `Error`. `String(it)` on one of those is "[object Object]", which is
// how an engine failure turns into a dialog that says nothing. This is the one
// place that knows the shape, so no component has to guess.

/** Where the engine says a failure belongs on screen. */
export type Surface = "transfer-row" | "banner" | "field" | "trust-prompt";

/** The host-key detail behind a trust prompt or a changed-key banner. */
export interface HostKeyProblem {
  host: string;
  port: number;
  algorithm: string;
  /** `SHA256:…`, as `ssh-keygen -lf` prints it. */
  fingerprint: string;
  /** The previously accepted fingerprint. Present only when the key changed. */
  expected?: string;
}

/** An engine failure, however it reached us. */
export interface EngineFailure {
  message: string;
  surface: Surface;
  transient: boolean;
  hostKey?: HostKeyProblem;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/**
 * Normalises anything thrown by a backend call into one shape.
 *
 * Falls back to `transfer-row` for a failure that carries no surface — a
 * thrown `Error` from the fixture backend, say. That is the least disruptive
 * of the four: it does not raise a banner or open a prompt on a value that was
 * never classified.
 */
export function engineFailure(error: unknown): EngineFailure {
  if (isRecord(error) && typeof error.message === "string") {
    const surface = typeof error.surface === "string" ? (error.surface as Surface) : "transfer-row";
    const hostKey = isRecord(error.hostKey)
      ? {
          host: String(error.hostKey.host ?? ""),
          port: Number(error.hostKey.port ?? 22),
          algorithm: String(error.hostKey.algorithm ?? ""),
          fingerprint: String(error.hostKey.fingerprint ?? ""),
          expected:
            typeof error.hostKey.expected === "string" ? error.hostKey.expected : undefined,
        }
      : undefined;
    return {
      message: error.message,
      surface,
      transient: error.transient === true,
      hostKey,
    };
  }
  return { message: String(error), surface: "transfer-row", transient: false };
}
