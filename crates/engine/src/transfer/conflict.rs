//! What to do when something is already there.
//!
//! Kept apart from the worker because the naming rule is fiddly and entirely
//! testable, and because the *decision* belongs to the user while the *name*
//! belongs here.

use serde::{Deserialize, Serialize};

/// What the user chose for a destination that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    /// Overwrite what is there.
    Replace,
    /// Write alongside it under a new name.
    KeepBoth,
    /// Do not transfer this one.
    Skip,
}

/// A destination that already exists, awaiting a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    /// The transfer this is about.
    pub transfer_id: String,
    /// The name in the way.
    pub name: String,
    /// Full destination path.
    pub dest_path: String,
    /// What `KeepBoth` would call it, so the prompt can show the actual name.
    pub keep_both_as: String,
}

/// The name `Keep both` should use, given the names already taken.
///
/// The suffix goes *before* the extension — `report 2.pdf`, not
/// `report.pdf 2` — because the extension is what decides which application
/// opens the file, and a name that loses it is a file the user cannot open.
///
/// Numbering starts at 2 and skips whatever is taken, so repeatedly keeping
/// both produces ` 2`, ` 3`, ` 4` rather than stopping at the first collision.
#[must_use]
pub fn keep_both_name(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let (stem, extension) = split_extension(name);
    for suffix in 2..10_000_u32 {
        let candidate = match extension {
            Some(extension) => format!("{stem} {suffix}.{extension}"),
            None => format!("{stem} {suffix}"),
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    // Ten thousand collisions is not a naming problem any more. Falling back
    // to something certainly unique beats looping forever.
    match extension {
        Some(extension) => format!("{stem} copy.{extension}"),
        None => format!("{stem} copy"),
    }
}

/// Split a filename into its stem and extension.
///
/// A leading dot does not begin an extension — `.bashrc` is a hidden file
/// named `bashrc`, and `Keep both` on it must produce `.bashrc 2` rather than
/// `. 2.bashrc`. The same rule the kind table uses.
fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            (stem, Some(extension))
        }
        _ => (name, None),
    }
}

/// Replace the final component of a path.
#[must_use]
pub fn with_name(path: &str, name: &str) -> String {
    match path.rfind('/') {
        Some(cut) => format!("{}/{name}", &path[..cut]),
        None => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{keep_both_name, split_extension, with_name};

    fn nothing_taken(_: &str) -> bool {
        false
    }

    #[test]
    fn the_suffix_goes_before_the_extension() {
        // `report.pdf 2` would be a file the user cannot open, because the
        // extension is what decides which application opens it.
        assert_eq!(keep_both_name("report.pdf", &nothing_taken), "report 2.pdf");
    }

    #[test]
    fn numbering_skips_what_is_already_taken() {
        let taken = |name: &str| matches!(name, "report 2.pdf" | "report 3.pdf");
        assert_eq!(keep_both_name("report.pdf", &taken), "report 4.pdf");
    }

    #[test]
    fn a_name_without_an_extension_still_gets_a_suffix() {
        assert_eq!(keep_both_name("README", &nothing_taken), "README 2");
    }

    #[test]
    fn a_dotfile_keeps_its_leading_dot() {
        // `.bashrc` is a hidden file named bashrc, not an extension.
        assert_eq!(keep_both_name(".bashrc", &nothing_taken), ".bashrc 2");
        assert_eq!(split_extension(".bashrc"), (".bashrc", None));
    }

    #[test]
    fn only_the_last_extension_is_kept_intact() {
        // `.tar.gz` is two extensions and the rule is about the last one, so
        // this is `backup.tar 2.gz`. Not beautiful, but predictable, and it
        // still opens.
        assert_eq!(
            keep_both_name("backup.tar.gz", &nothing_taken),
            "backup.tar 2.gz"
        );
    }

    #[test]
    fn a_pathological_number_of_collisions_terminates() {
        assert_eq!(keep_both_name("a.txt", &|_| true), "a copy.txt");
    }

    #[test]
    fn renaming_replaces_only_the_final_component() {
        assert_eq!(with_name("a/b/c.txt", "c 2.txt"), "a/b/c 2.txt");
        assert_eq!(with_name("c.txt", "c 2.txt"), "c 2.txt");
    }
}
