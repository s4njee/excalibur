// The seam every filesystem and network call crosses.
//
// Three implementations, chosen at runtime by `defaultBackend`:
//   `tauri.ts`   — the real one, `invoke` into Rust
//   `fixture.ts` — the in-browser preview and every component test
//   `guard.ts`   — a wrapper that refuses writes on a read-only mount
// The interface itself, and the values crossing it, are in `types.ts`.

import { fixtureBackend } from "./fixture";
import { inTauri, tauriBackend } from "./tauri";
import type { Backend } from "./types";

export type {
  Backend,
  HttpClientSettings,
  Spec,
  UserFolders,
  WindowDragEvent,
} from "./types";
export { fixtureBackend } from "./fixture";
export { writeGuarded } from "./guard";

export function defaultBackend(): Backend {
  return inTauri() ? tauriBackend() : fixtureBackend();
}
