// Small persisted values that are not bookmarks.
//
// The same Tauri-or-browser split the bookmark store uses, but keyed and
// untyped, because the things that go here — a sort, a set of column widths, a
// row layout — arrive one story at a time. E17-S1 is where this grows a real
// schema; until then every reader is expected to validate what it gets back,
// which it should do anyway: the file is user-editable and outlives upgrades.

export interface Prefs {
  get(key: string): Promise<unknown>;
  set(key: string, value: unknown): Promise<void>;
}

const FILE = "prefs.json";

function tauriPrefs(): Prefs {
  const open = async () => {
    const { load } = await import("@tauri-apps/plugin-store");
    return load(FILE, { autoSave: true });
  };
  return {
    async get(key) {
      const store = await open();
      return store.get(key);
    },
    async set(key, value) {
      const store = await open();
      await store.set(key, value);
      await store.save();
    },
  };
}

function browserPrefs(): Prefs {
  return {
    async get(key) {
      try {
        const raw = window.localStorage.getItem(`prefs:${key}`);
        return raw === null ? undefined : JSON.parse(raw);
      } catch {
        // A preference that cannot be read is not worth failing over. The
        // caller's default is a perfectly good answer.
        return undefined;
      }
    },
    async set(key, value) {
      try {
        window.localStorage.setItem(`prefs:${key}`, JSON.stringify(value));
      } catch {
        // Private browsing, or a full quota. Losing a column width is not
        // worth an error the user has to dismiss.
      }
    },
  };
}

/** Whether the shell is around us, or this is the browser preview. */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function defaultPrefs(): Prefs {
  return inTauri() ? tauriPrefs() : browserPrefs();
}

/** Nothing persists. For tests, and for previews that should start clean. */
export function memoryPrefs(): Prefs {
  const held = new Map<string, unknown>();
  return {
    async get(key) {
      return held.get(key);
    },
    async set(key, value) {
      held.set(key, value);
    },
  };
}
