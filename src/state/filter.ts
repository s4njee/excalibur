// The toolbar's filter.
//
// Client-side over the listing already on screen: it updates per keystroke and
// never re-lists, because re-listing on every keystroke would put a directory
// read behind a text field.
//
// "Case-insensitive and diacritic-insensitive" is the whole requirement, and
// the second half is the one that is easy to get wrong. A user typing `resume`
// expects to find `Résumé`, and a user typing `Résumé` expects to find it too.

import type { Entry } from "./types";

/**
 * Letters that carry a diacritic Unicode will not decompose.
 *
 * NFD splits `é` into `e` + a combining accent, which strips cleanly. It does
 * nothing for `ø`, `ð` or `ß`, which are separate letters rather than a base
 * plus a mark — so searching `soren` would miss `Søren` without this. Not an
 * exhaustive transliteration table and not meant to be: these are the Latin
 * letters common enough to turn up in filenames.
 */
const FOLDED: Record<string, string> = {
  ø: "o",
  đ: "d",
  ð: "d",
  ł: "l",
  ß: "ss",
  æ: "ae",
  œ: "oe",
  þ: "th",
  ħ: "h",
  ŧ: "t",
  ı: "i",
};

/**
 * Reduce a string to what matching should compare.
 *
 * Lower-cased, decomposed, stripped of combining marks, then the letters NFD
 * cannot handle replaced by hand.
 */
export function fold(text: string): string {
  const lowered = text.toLowerCase();
  let out = "";
  for (const character of lowered) {
    out += FOLDED[character] ?? character;
  }
  return out.normalize("NFD").replace(/\p{Diacritic}/gu, "");
}

/** Whether one name matches a filter. An empty filter matches everything. */
export function matches(name: string, needle: string): boolean {
  if (needle === "") return true;
  return fold(name).includes(fold(needle));
}

/** The entries a filter leaves. */
export function filterEntries(entries: Entry[], needle: string): Entry[] {
  if (needle.trim() === "") return entries;
  return entries.filter((entry) => matches(entry.name, needle));
}
