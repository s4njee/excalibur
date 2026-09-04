// Presentation helpers. Kept apart from the components so the exact strings
// the design specifies are testable without rendering anything.

/**
 * Bytes as the Size column shows them, in decimal units.
 *
 * One decimal place, dropped when it would be `.0`: the design shows both
 * "2.4 TB" and "16 TB" in the same sentence, so a fixed number of decimals
 * matches neither. Three digits displace the decimal entirely ("999 GB").
 */
export function formatSize(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  // Only an exact whole number loses its decimal: "16 TB" is exactly 16, while
  // 44.98 GB is not 45 and should not claim to be.
  const whole = Number.isInteger(value);
  const rendered = value >= 100 || whole ? value.toFixed(0) : value.toFixed(1);
  return `${rendered} ${units[unit]}`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** The Modified column's format: `31 Aug 2026, 18:24`. */
export function formatModified(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  const day = date.getDate();
  const month = MONTHS[date.getMonth()];
  const hours = `${date.getHours()}`.padStart(2, "0");
  const minutes = `${date.getMinutes()}`.padStart(2, "0");
  return `${day} ${month} ${date.getFullYear()}, ${hours}:${minutes}`;
}

/** The short form the dense row layout uses: `31 Aug 18:24`. */
export function formatModifiedShort(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  const hours = `${date.getHours()}`.padStart(2, "0");
  const minutes = `${date.getMinutes()}`.padStart(2, "0");
  return `${date.getDate()} ${MONTHS[date.getMonth()]} ${hours}:${minutes}`;
}

/**
 * The mono grid's size: `48.2G`, `2.0K`, `812`.
 *
 * One character of unit and no space, because `1h` aligns every column by
 * character rather than by pixel — a `48.2 GB` in that grid would push the
 * next column out of true.
 */
export function formatSizeTerse(bytes: number): string {
  if (bytes < 1000) return `${bytes}`;
  const units = ["K", "M", "G", "T", "P"];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  // Three significant figures at most, so the column stays five characters
  // wide: `999K`, `48.2G`, `1.0T`.
  const rendered = value >= 100 ? value.toFixed(0) : value.toFixed(1);
  return `${rendered}${units[unit]}`;
}

/**
 * The mono grid's date: `08-31 18:24`.
 *
 * No year, like `ls`, because every row is the same width and the year is the
 * part that is usually the same.
 */
export function formatModifiedGrid(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  const month = `${date.getMonth() + 1}`.padStart(2, "0");
  const day = `${date.getDate()}`.padStart(2, "0");
  const hours = `${date.getHours()}`.padStart(2, "0");
  const minutes = `${date.getMinutes()}`.padStart(2, "0");
  return `${month}-${day} ${hours}:${minutes}`;
}

/**
 * The extension the name-led layout shows: `mkv`, or `—` when there is none.
 *
 * From the name rather than the engine's kind label, because the design's
 * second line reads `48.2 GB · mkv · 31 Aug 18:24` — a terse token, not the
 * prose "Matroska Video" the Kind column carries.
 */
export function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  // A leading dot is a hidden file, not an extension: `.DS_Store` has none.
  if (dot <= 0 || dot === name.length - 1) return "—";
  return name.slice(dot + 1).toLowerCase();
}

/**
 * The status bar's counts: `13 items · 1 selected · 48.2 GB`.
 *
 * The size is of the *selection*, not the directory — the design is explicit
 * about that, and showing the total instead would be a plausible-looking lie.
 *
 * When a filter is hiding rows the count says `3 of 13 items`. The AC asks for
 * counts that reflect the filtered set, and a bare `3 items` does reflect it —
 * but it also reads exactly like a directory that contains three files, which
 * is the one thing the user must not conclude while a filter is on.
 */
export function formatCounts(
  items: number,
  selected: number,
  selectedBytes: number,
  total?: number,
): string {
  const filtered = total !== undefined && total !== items;
  const parts = [
    filtered
      ? `${items} of ${total} item${total === 1 ? "" : "s"}`
      : `${items} item${items === 1 ? "" : "s"}`,
  ];
  if (selected > 0) {
    parts.push(`${selected} selected`, formatSize(selectedBytes));
  }
  return parts.join(" · ");
}

/** The capacity phrase: `2.4 TB free of 16 TB`. */
export function formatCapacity(free: number, total: number): string {
  return `${formatSize(free)} free of ${formatSize(total)}`;
}

/**
 * A duration as the transfer rows show it: `3m 04s`, `1h 02m`, `12s`.
 *
 * Two units at most. "1h 02m 33s" is more precise than anyone reading a
 * progress row needs, and the extra field makes the ones that matter harder
 * to find.
 */
export function formatDuration(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  if (whole < 60) return `${whole}s`;
  const minutes = Math.floor(whole / 60);
  if (minutes < 60) return `${minutes}m ${String(whole % 60).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/** Throughput as the rows show it: `112 MB/s`. */
export function formatRate(bytesPerSecond: number): string {
  return `${formatSize(bytesPerSecond)}/s`;
}

/**
 * The right-hand status string on a transfer row.
 *
 * The design gives three shapes, and this produces exactly those:
 * `41.2 / 61.7 GB · 112 MB/s · 3m 04s` while running, `queued` while waiting,
 * `done · 84 KB` when finished. Anything it cannot say it leaves out rather
 * than filling with a placeholder — a row reading `· — · —` is noise.
 */
export function formatTransferStatus(transfer: {
  status: string;
  bytesDone: number;
  bytesTotal: number;
  rate: number | null;
  etaSeconds: number | null;
}): string {
  switch (transfer.status) {
    case "queued":
      return "queued";
    case "paused":
      return "paused";
    case "cancelled":
      return "cancelled";
    case "done":
      return `done · ${formatSize(transfer.bytesDone)}`;
    case "failed":
      return "failed";
    default:
      break;
  }
  const parts: string[] = [];
  if (transfer.bytesTotal > 0) {
    // The unit appears once, on the total: "41.2 / 61.7 GB" rather than
    // "41.2 GB / 61.7 GB", which is what the design shows.
    const total = formatSize(transfer.bytesTotal);
    const unit = total.slice(total.indexOf(" "));
    const done = formatSize(transfer.bytesDone);
    const doneNumber = done.endsWith(unit) ? done.slice(0, -unit.length) : done;
    parts.push(`${doneNumber} / ${total}`);
  } else {
    parts.push(formatSize(transfer.bytesDone));
  }
  if (transfer.rate !== null && transfer.rate > 0) parts.push(formatRate(transfer.rate));
  if (transfer.etaSeconds !== null) parts.push(formatDuration(transfer.etaSeconds));
  return parts.join(" · ");
}
