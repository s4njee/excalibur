import { createMemo, createResource, createSignal, Show } from "solid-js";
import { FileList, shellStyles as styles, Sidebar, StatusBar, TitleBar, Toolbar } from "./components/Shell";
import { defaultBackend, fixtureBackend, type Backend } from "./state/backend";
import type { Bookmark, Entry, Theme } from "./state/types";

/**
 * The sidebar's contents. Persisted bookmarks arrive with E3-S2; until then
 * these are the rows the design shows, including the unreachable one, so the
 * states that are easy to forget are visible from the start.
 */
const BOOKMARKS: Bookmark[] = [
  {
    id: "nas-media",
    name: "media",
    protocol: "smb",
    host: "nas.local",
    share: "media",
    mountPreference: "rw",
    mounted: true,
    reachable: true,
  },
  {
    id: "backups",
    name: "backups",
    protocol: "smb",
    host: "nas.local",
    share: "backups",
    mountPreference: "rw",
    mounted: false,
    reachable: true,
  },
  {
    id: "scratch",
    name: "scratch",
    protocol: "nfs",
    host: "10.0.0.4",
    share: "/export/scratch",
    mountPreference: "rw",
    mounted: false,
    reachable: true,
  },
  {
    id: "archive",
    name: "archive",
    protocol: "sftp",
    host: "vault.example",
    share: "/srv/archive",
    mountPreference: "ro",
    mounted: false,
    reachable: false,
  },
];

/** Sorted the way the list is: folders first, then by name. */
function order(entries: Entry[], showHidden: boolean): Entry[] {
  return entries
    .filter((entry) => showHidden || !entry.hidden)
    .slice()
    .sort((left, right) => {
      const leftIsDir = left.kind === "dir";
      const rightIsDir = right.kind === "dir";
      if (leftIsDir !== rightIsDir) return leftIsDir ? -1 : 1;
      return left.name.localeCompare(right.name, undefined, { numeric: true });
    });
}

export default function App(props: { backend?: Backend; theme?: Theme; readOnly?: boolean }) {
  // `readOnly` exists so the design's second screen can be rendered from the
  // same component tree rather than a separate mock.
  const backend =
    props.backend ?? (props.readOnly ? fixtureBackend({ readOnly: true }) : defaultBackend());
  const [listing] = createResource(() => backend.listDir(""));
  const [filter, setFilter] = createSignal("");
  const [showHidden] = createSignal(true);
  const [selected, setSelected] = createSignal<Set<string>>(new Set(["Arrival.2016.mkv"]));

  const entries = createMemo(() => {
    const data = listing();
    if (!data) return [];
    const needle = filter().toLowerCase();
    const matching = needle
      ? data.entries.filter((entry) => entry.name.toLowerCase().includes(needle))
      : data.entries;
    return order(matching, showHidden());
  });

  const selectedBytes = createMemo(() =>
    entries()
      .filter((entry) => selected().has(entry.path))
      .reduce((total, entry) => total + entry.size, 0),
  );

  // The two M0 screens describe different mounts, so the chrome that names the
  // mount has to follow them rather than being fixed.
  const host = () => (props.readOnly ? "10.0.0.4" : "nas.local");
  const share = () => (props.readOnly ? "scratch" : "media");
  const scheme = () => (props.readOnly ? "nfs" : "smb");
  const segments = () => (props.readOnly ? ["export", "scratch"] : ["media"]);
  const mountPath = () => (props.readOnly ? "/export/scratch" : "/media");

  const select = (path: string, additive: boolean) => {
    setSelected((current) => {
      const next = additive ? new Set(current) : new Set<string>();
      if (additive && current.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  return (
    <Show when={listing()} fallback={<div class={styles.window} />}>
      {(data) => (
        <div class={styles.window}>
          <TitleBar share={share()} host={host()} />
          <Toolbar
            mount={data().mount}
            scheme={scheme()}
            host={host()}
            segments={segments()}
            hasSelection={selected().size > 0}
            filter={filter()}
            onFilter={setFilter}
          />
          <div class={styles.body}>
            <Sidebar
              bookmarks={BOOKMARKS}
              activeId="nas-media"
              writable={data().mount.writable}
            />
            <div class={styles.listColumn}>
              <FileList entries={entries()} selected={selected} onSelect={select} />
              <StatusBar
                mount={data().mount}
                path={mountPath()}
                items={entries().length}
                selected={selected().size}
                selectedBytes={selectedBytes()}
              />
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
