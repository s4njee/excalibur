//! Extension to human-readable kind, for the Kind column.
//!
//! Deliberately a table over names rather than content sniffing: a listing must
//! not read any file. The design's Kind column is a label, so a name is the
//! right amount of evidence for it — and reading the first bytes of every entry
//! to fill a column would turn one round trip into hundreds.
//!
//! The table carries a [`Category`] beside each label because three features
//! need the same answer to "what sort of thing is this": the Kind column (this
//! label), the row glyph (E0-S3), and Quick Look's choice of viewer (E11).
//! Three tables would drift; one cannot.

use serde::{Deserialize, Serialize};

/// What a file is, coarsely enough to choose an icon and a viewer.
///
/// Not a MIME type. The distinctions here are the ones the UI acts on, which is
/// why `Text` and `Code` are separate (a code preview wants line numbers and a
/// log does not) while every archive format is one thing (nothing opens them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// A directory.
    Folder,
    /// A symbolic link.
    Alias,
    Video,
    Audio,
    Image,
    Pdf,
    /// Plain text, including data formats a person can read.
    Text,
    /// Source code. Previews the same way as text, with line numbers.
    Code,
    /// A word processor, spreadsheet or presentation file.
    Document,
    Archive,
    /// A disk or disc image.
    Disk,
    /// An executable or bundle.
    Application,
    Font,
    /// A database or serialised dataset — opaque, but not an archive.
    Data,
    /// Known extension, nothing above fits; or no extension at all.
    Other,
}

impl Category {
    /// Whether Quick Look has a viewer for this (E11).
    ///
    /// Lives here rather than in the panel so that adding a category forces a
    /// decision about previewing it, instead of silently defaulting to "no".
    #[must_use]
    pub const fn previewable(self) -> bool {
        matches!(
            self,
            Self::Video | Self::Audio | Self::Image | Self::Pdf | Self::Text | Self::Code
        )
    }

    /// The Uniform Type Identifier a file promise is offered under (E24-S1).
    ///
    /// **Why the category and not the extension.** A promise's type is what
    /// the receiving app matches against when it decides whether it will take
    /// the drag at all, and what Finder draws the dragged badge from. It is
    /// not what names the file — that is [`crate::drag::Promise::name`], which
    /// carries the extension verbatim — so the answer only has to be true, not
    /// maximally specific. Declaring one system type per category is a table of
    /// fifteen answers that can be read against Apple's own list; declaring one
    /// per extension would be two hundred and fifty, each of them a guess that
    /// nothing in the app would ever notice being wrong.
    ///
    /// Every string here is a type Apple declares. A made-up identifier is not
    /// a harmless placeholder: an unknown UTI makes a drag that no app accepts.
    #[must_use]
    pub const fn uti(self) -> &'static str {
        match self {
            Self::Folder => "public.folder",
            Self::Alias => "public.symlink",
            Self::Video => "public.movie",
            Self::Audio => "public.audio",
            Self::Image => "public.image",
            Self::Pdf => "com.adobe.pdf",
            Self::Text => "public.plain-text",
            Self::Code => "public.source-code",
            Self::Archive => "public.archive",
            Self::Disk => "public.disk-image",
            Self::Application => "public.executable",
            Self::Font => "public.font",
            // A word processor file, a database and an unrecognised extension
            // have no abstract type in common beyond "a file with bytes in
            // it", and `public.data` is exactly that. Reaching for
            // `public.content` instead would claim these are all things an app
            // opens as documents, which of `Data` is simply untrue.
            Self::Document | Self::Data | Self::Other => "public.data",
        }
    }
}

/// One row: extension, the label to show, and what sort of thing it is.
type Row = (&'static str, &'static str, Category);

/// The kind table, **sorted by extension** so the lookup can bisect it.
///
/// A test asserts both the ordering and the size, so neither can rot: an
/// unsorted insertion would silently make some extensions unfindable.
const TABLE: &[Row] = &[
    ("7z", "7-Zip Archive", Category::Archive),
    ("aac", "AAC Audio", Category::Audio),
    ("ac3", "Dolby Digital Audio", Category::Audio),
    ("ai", "Illustrator Document", Category::Image),
    ("aif", "AIFF Audio", Category::Audio),
    ("aiff", "AIFF Audio", Category::Audio),
    ("alac", "Apple Lossless Audio", Category::Audio),
    ("amr", "AMR Audio", Category::Audio),
    ("ape", "Monkey's Audio", Category::Audio),
    ("apk", "Android Package", Category::Archive),
    ("app", "Application", Category::Application),
    ("appimage", "AppImage Application", Category::Application),
    ("ar", "Unix Archive", Category::Archive),
    ("arw", "Sony Raw Image", Category::Image),
    ("asc", "PGP Signature", Category::Text),
    ("asf", "ASF Video", Category::Video),
    ("asm", "Assembly Source", Category::Code),
    ("ass", "Advanced SubStation Text", Category::Text),
    ("avi", "AVI Video", Category::Video),
    ("avif", "AVIF Image", Category::Image),
    ("avro", "Avro Data", Category::Data),
    ("azw3", "Kindle Book", Category::Document),
    ("bak", "Backup File", Category::Data),
    ("bash", "Shell Script", Category::Code),
    ("bat", "Batch Script", Category::Code),
    ("bin", "Binary Image", Category::Disk),
    ("bmp", "Bitmap Image", Category::Image),
    ("bz2", "Bzip2 Archive", Category::Archive),
    ("c", "C Source", Category::Code),
    ("cab", "Cabinet Archive", Category::Archive),
    ("cc", "C++ Source", Category::Code),
    ("cer", "Certificate", Category::Text),
    ("cfg", "Configuration", Category::Text),
    ("cjs", "JavaScript Source", Category::Code),
    ("clj", "Clojure Source", Category::Code),
    ("cmake", "CMake Script", Category::Code),
    ("cmd", "Batch Script", Category::Code),
    ("com", "DOS Executable", Category::Application),
    ("conf", "Configuration", Category::Text),
    ("cpio", "cpio Archive", Category::Archive),
    ("cpp", "C++ Source", Category::Code),
    ("cr2", "Canon Raw Image", Category::Image),
    ("cr3", "Canon Raw Image", Category::Image),
    ("crate", "Rust Crate", Category::Archive),
    ("crt", "Certificate", Category::Text),
    ("cs", "C# Source", Category::Code),
    ("csv", "CSV Text", Category::Text),
    ("cue", "Cue Sheet", Category::Text),
    ("cxx", "C++ Source", Category::Code),
    ("dart", "Dart Source", Category::Code),
    ("dat", "Data File", Category::Data),
    ("db", "Database", Category::Data),
    ("deb", "Debian Package", Category::Archive),
    ("divx", "DivX Video", Category::Video),
    ("djvu", "DjVu Document", Category::Document),
    ("dll", "Dynamic Library", Category::Application),
    ("dmg", "Disk Image", Category::Disk),
    ("dng", "Digital Negative Image", Category::Image),
    ("doc", "Word Document", Category::Document),
    ("dockerfile", "Dockerfile", Category::Code),
    ("docx", "Word Document", Category::Document),
    ("dsf", "DSD Audio", Category::Audio),
    ("dts", "DTS Audio", Category::Audio),
    ("dylib", "Dynamic Library", Category::Application),
    ("env", "Environment File", Category::Text),
    ("eot", "Embedded OpenType Font", Category::Font),
    ("eps", "PostScript Image", Category::Image),
    ("epub", "EPUB Book", Category::Document),
    ("erl", "Erlang Source", Category::Code),
    ("ex", "Elixir Source", Category::Code),
    ("exe", "Windows Executable", Category::Application),
    ("exs", "Elixir Script", Category::Code),
    ("f4v", "Flash Video", Category::Video),
    ("fish", "Fish Script", Category::Code),
    ("flac", "FLAC Audio", Category::Audio),
    ("flv", "Flash Video", Category::Video),
    ("fs", "F# Source", Category::Code),
    ("gem", "Ruby Gem", Category::Archive),
    ("gif", "GIF Image", Category::Image),
    ("go", "Go Source", Category::Code),
    ("gpg", "PGP Data", Category::Other),
    ("gradle", "Gradle Script", Category::Code),
    ("graphql", "GraphQL Schema", Category::Code),
    ("gz", "Gzip Archive", Category::Archive),
    ("h", "C Header", Category::Code),
    ("h5", "HDF5 Data", Category::Data),
    ("hdf5", "HDF5 Data", Category::Data),
    ("heic", "HEIF Image", Category::Image),
    ("heif", "HEIF Image", Category::Image),
    ("hpp", "C++ Header", Category::Code),
    ("hs", "Haskell Source", Category::Code),
    ("htm", "HTML Document", Category::Code),
    ("html", "HTML Document", Category::Code),
    ("ico", "Icon Image", Category::Image),
    ("ics", "Calendar", Category::Text),
    ("idx", "Subtitle Index", Category::Text),
    ("img", "Disk Image", Category::Disk),
    ("ini", "Configuration", Category::Text),
    ("iso", "Disc Image", Category::Disk),
    ("jar", "Java Archive", Category::Archive),
    ("java", "Java Source", Category::Code),
    ("jl", "Julia Source", Category::Code),
    ("jpeg", "JPEG Image", Category::Image),
    ("jpg", "JPEG Image", Category::Image),
    ("js", "JavaScript Source", Category::Code),
    ("json", "JSON Text", Category::Text),
    ("jsx", "JavaScript Source", Category::Code),
    ("jxl", "JPEG XL Image", Category::Image),
    ("key", "Keynote Presentation", Category::Document),
    ("kt", "Kotlin Source", Category::Code),
    ("less", "Less Stylesheet", Category::Code),
    ("log", "Log File", Category::Text),
    ("lua", "Lua Source", Category::Code),
    ("lz4", "LZ4 Archive", Category::Archive),
    ("lzma", "LZMA Archive", Category::Archive),
    ("m", "Objective-C Source", Category::Code),
    ("m2ts", "MPEG Transport Video", Category::Video),
    ("m4a", "AAC Audio", Category::Audio),
    ("m4v", "MPEG-4 Video", Category::Video),
    ("makefile", "Makefile", Category::Code),
    ("markdown", "Markdown Text", Category::Text),
    ("mat", "MATLAB Data", Category::Data),
    ("md", "Markdown Text", Category::Text),
    ("mdb", "Access Database", Category::Data),
    ("mid", "MIDI Audio", Category::Audio),
    ("midi", "MIDI Audio", Category::Audio),
    ("mjs", "JavaScript Source", Category::Code),
    ("mk", "Makefile", Category::Code),
    ("mkv", "Matroska Video", Category::Video),
    ("ml", "OCaml Source", Category::Code),
    ("mm", "Objective-C++ Source", Category::Code),
    ("mobi", "Kindle Book", Category::Document),
    ("mov", "QuickTime Movie", Category::Video),
    ("mp3", "MP3 Audio", Category::Audio),
    ("mp4", "MPEG-4 Video", Category::Video),
    ("mpeg", "MPEG Video", Category::Video),
    ("mpg", "MPEG Video", Category::Video),
    ("msi", "Windows Installer", Category::Archive),
    ("mts", "AVCHD Video", Category::Video),
    ("nef", "Nikon Raw Image", Category::Image),
    ("nfo", "Info Text", Category::Text),
    ("nim", "Nim Source", Category::Code),
    ("npy", "NumPy Data", Category::Data),
    ("nupkg", "NuGet Package", Category::Archive),
    ("odp", "OpenDocument Presentation", Category::Document),
    ("ods", "OpenDocument Spreadsheet", Category::Document),
    ("odt", "OpenDocument Text", Category::Document),
    ("oga", "Ogg Audio", Category::Audio),
    ("ogg", "Ogg Audio", Category::Audio),
    ("ogv", "Ogg Video", Category::Video),
    ("opus", "Opus Audio", Category::Audio),
    ("orc", "ORC Data", Category::Data),
    ("orf", "Olympus Raw Image", Category::Image),
    ("otf", "OpenType Font", Category::Font),
    ("p12", "PKCS#12 Keystore", Category::Other),
    ("pages", "Pages Document", Category::Document),
    ("parquet", "Parquet Data", Category::Data),
    ("pdf", "PDF Document", Category::Pdf),
    ("pem", "PEM Certificate", Category::Text),
    ("pfb", "PostScript Font", Category::Font),
    ("pfx", "PKCS#12 Keystore", Category::Other),
    ("pgp", "PGP Data", Category::Other),
    ("php", "PHP Source", Category::Code),
    ("pkg", "Installer Package", Category::Archive),
    ("pkl", "Pickle Data", Category::Data),
    ("pl", "Perl Source", Category::Code),
    ("plist", "Property List", Category::Text),
    ("png", "PNG Image", Category::Image),
    ("ppt", "PowerPoint Presentation", Category::Document),
    ("pptx", "PowerPoint Presentation", Category::Document),
    ("proto", "Protocol Buffer", Category::Code),
    ("ps1", "PowerShell Script", Category::Code),
    ("psd", "Photoshop Document", Category::Image),
    ("py", "Python Source", Category::Code),
    ("qcow2", "Disk Image", Category::Disk),
    ("r", "R Source", Category::Code),
    ("rar", "RAR Archive", Category::Archive),
    ("raw", "Raw Image", Category::Image),
    ("rb", "Ruby Source", Category::Code),
    ("rmvb", "RealMedia Video", Category::Video),
    ("rpm", "RPM Package", Category::Archive),
    ("rs", "Rust Source", Category::Code),
    ("rtf", "Rich Text", Category::Document),
    ("rw2", "Panasonic Raw Image", Category::Image),
    ("s", "Assembly Source", Category::Code),
    ("sass", "Sass Stylesheet", Category::Code),
    ("scala", "Scala Source", Category::Code),
    ("scss", "Sass Stylesheet", Category::Code),
    ("sh", "Shell Script", Category::Code),
    ("sig", "Signature", Category::Other),
    ("sit", "StuffIt Archive", Category::Archive),
    ("so", "Shared Library", Category::Application),
    ("sql", "SQL Source", Category::Code),
    ("sqlite", "SQLite Database", Category::Data),
    ("sqlite3", "SQLite Database", Category::Data),
    ("srt", "SubRip Text", Category::Text),
    ("ssa", "SubStation Text", Category::Text),
    ("sub", "Subtitle Text", Category::Text),
    ("svelte", "Svelte Component", Category::Code),
    ("svg", "SVG Image", Category::Image),
    ("swift", "Swift Source", Category::Code),
    ("tar", "TAR Archive", Category::Archive),
    ("tbz", "Bzip2 Archive", Category::Archive),
    ("tex", "TeX Document", Category::Code),
    ("text", "Plain Text", Category::Text),
    ("tgz", "Gzip Archive", Category::Archive),
    ("tif", "TIFF Image", Category::Image),
    ("tiff", "TIFF Image", Category::Image),
    ("toast", "Disc Image", Category::Disk),
    ("toml", "TOML Text", Category::Text),
    ("torrent", "Torrent File", Category::Other),
    ("ts", "TypeScript Source", Category::Code),
    ("tsv", "TSV Text", Category::Text),
    ("tsx", "TypeScript Source", Category::Code),
    ("ttf", "TrueType Font", Category::Font),
    ("txt", "Plain Text", Category::Text),
    ("txz", "XZ Archive", Category::Archive),
    ("v", "Verilog Source", Category::Code),
    ("vcf", "Contact Card", Category::Text),
    ("vdi", "Disk Image", Category::Disk),
    ("vhd", "Disk Image", Category::Disk),
    ("vhdx", "Disk Image", Category::Disk),
    ("vim", "Vim Script", Category::Code),
    ("vmdk", "Disk Image", Category::Disk),
    ("vob", "DVD Video", Category::Video),
    ("vtt", "WebVTT Text", Category::Text),
    ("vue", "Vue Component", Category::Code),
    ("war", "Web Archive", Category::Archive),
    ("wasm", "WebAssembly Module", Category::Application),
    ("wav", "WAV Audio", Category::Audio),
    ("webm", "WebM Video", Category::Video),
    ("webp", "WebP Image", Category::Image),
    ("whl", "Python Wheel", Category::Archive),
    ("wma", "Windows Media Audio", Category::Audio),
    ("wmv", "Windows Media Video", Category::Video),
    ("woff", "Web Font", Category::Font),
    ("woff2", "Web Font", Category::Font),
    ("xcf", "GIMP Image", Category::Image),
    ("xls", "Excel Spreadsheet", Category::Document),
    ("xlsx", "Excel Spreadsheet", Category::Document),
    ("xml", "XML Text", Category::Text),
    ("xz", "XZ Archive", Category::Archive),
    ("yaml", "YAML Text", Category::Text),
    ("yml", "YAML Text", Category::Text),
    ("zig", "Zig Source", Category::Code),
    ("zip", "ZIP Archive", Category::Archive),
    ("zsh", "Shell Script", Category::Code),
    ("zst", "Zstandard Archive", Category::Archive),
];

/// Keys that name a whole file rather than a suffix.
///
/// `Makefile` and `Dockerfile` have no extension but are not mysteries. They
/// are kept in the same table so there is still one place to look, and listed
/// here so a whole-name lookup cannot accidentally match a file literally
/// called `gz`.
const WHOLE_NAME: &[&str] = &["dockerfile", "makefile"];

/// A label and a category: everything the UI needs to draw and route one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kind {
    /// What the Kind column shows.
    pub label: String,
    /// What sort of thing it is, for the glyph and for Quick Look.
    pub category: Category,
}

/// The lower-cased extension of a name, or `None` when it has none.
///
/// A leading dot does not begin an extension: `.bashrc` is a hidden file named
/// `bashrc`, not a `BASHRC File`. `rsplit_once` alone gets that wrong, and it
/// is the common case in every dotfile-heavy directory.
fn extension_of(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    if stem.is_empty() || extension.is_empty() {
        return None;
    }
    Some(extension.to_ascii_lowercase())
}

/// Classify a regular file by name.
#[must_use]
pub fn classify(name: &str) -> Kind {
    // A name with no extension carries no evidence, so it gets the neutral
    // label rather than a guess.
    let Some(extension) = extension_of(name) else {
        // Some files are known by their whole name, not by a suffix.
        let whole = name.to_ascii_lowercase();
        if WHOLE_NAME.contains(&whole.as_str()) {
            if let Ok(index) = TABLE.binary_search_by_key(&whole.as_str(), |(key, _, _)| key) {
                let (_, label, category) = TABLE[index];
                return Kind {
                    label: label.to_owned(),
                    category,
                };
            }
        }
        return Kind {
            label: "Document".to_owned(),
            category: Category::Other,
        };
    };
    match TABLE.binary_search_by_key(&extension.as_str(), |(key, _, _)| key) {
        Ok(index) => {
            let (_, label, category) = TABLE[index];
            Kind {
                label: label.to_owned(),
                category,
            }
        }
        // Named rather than called "Unknown": a user who sees "XYZ File" knows
        // more than one who sees "Unknown", and it costs nothing.
        Err(_) => Kind {
            label: format!("{} File", extension.to_ascii_uppercase()),
            category: Category::Other,
        },
    }
}

/// Classify any listing entry, including the ones an extension cannot answer.
///
/// Both backends call this rather than repeating the directory/symlink/hidden
/// match, which is exactly the sort of thing that drifts between two copies.
#[must_use]
pub fn classify_entry(name: &str, kind: crate::backend::EntryKind) -> Kind {
    use crate::backend::EntryKind;
    match kind {
        EntryKind::Dir => Kind {
            label: "Folder".to_owned(),
            category: Category::Folder,
        },
        EntryKind::Symlink => Kind {
            label: "Alias".to_owned(),
            category: Category::Alias,
        },
        _ if name.starts_with('.') => Kind {
            // The design labels dotfiles "Hidden" in the Kind column, but the
            // category still comes from the name: a hidden `.env` is text, and
            // Quick Look should be able to show it.
            label: "Hidden".to_owned(),
            category: classify(name).category,
        },
        _ => classify(name),
    }
}

/// The Kind label for a name, e.g. `"Matroska Video"`.
#[must_use]
pub fn label_for(name: &str) -> String {
    classify(name).label
}

#[cfg(test)]
mod tests {
    use super::{classify, classify_entry, label_for, Category, TABLE, WHOLE_NAME};
    use crate::backend::EntryKind;

    #[test]
    fn the_table_is_sorted_so_the_lookup_can_bisect_it() {
        // An out-of-order insertion would not fail to compile and would not
        // fail any label test except the one extension it hid.
        let mut previous = "";
        for (extension, _, _) in TABLE {
            assert!(
                *extension > previous,
                "{extension} is out of order after {previous}"
            );
            previous = extension;
        }
    }

    #[test]
    fn the_table_covers_at_least_the_hundred_and_fifty_the_design_asks_for() {
        assert!(TABLE.len() >= 150, "only {} extensions", TABLE.len());
    }

    #[test]
    fn every_label_is_titled_rather_than_a_bare_extension() {
        for (extension, label, _) in TABLE {
            assert!(!label.is_empty(), "{extension} has no label");
            if WHOLE_NAME.contains(extension) {
                // `Makefile` really is called Makefile. Everything else must
                // say more than the suffix already did.
                continue;
            }
            assert!(
                !label.eq_ignore_ascii_case(extension),
                "{extension} is labelled with itself"
            );
        }
    }

    #[test]
    fn a_whole_name_key_is_not_matched_as_a_suffix_by_accident() {
        // A file literally named `gz` is not a Gzip Archive; only the entries
        // in WHOLE_NAME may match without a dot.
        assert_eq!(label_for("gz"), "Document");
        assert_eq!(label_for("zip"), "Document");
    }

    #[test]
    fn known_extensions_get_the_designs_wording() {
        // The labels the handoff's mock shows, so they are what a pixel
        // comparison checks.
        assert_eq!(label_for("Arrival.2016.mkv"), "Matroska Video");
        assert_eq!(label_for("Arrival.2016.srt"), "SubRip Text");
    }

    #[test]
    fn an_unknown_extension_is_named_rather_than_called_unknown() {
        assert_eq!(label_for("archive.xyz"), "XYZ File");
        assert_eq!(classify("archive.xyz").category, Category::Other);
        // No extension at all is not the same as an unrecognised one.
        assert_eq!(label_for("README"), "Document");
    }

    #[test]
    fn a_leading_dot_does_not_begin_an_extension() {
        // `.bashrc` is a hidden file named bashrc, not a "BASHRC File".
        assert_eq!(label_for(".bashrc"), "Document");
        assert_eq!(label_for(".gitignore"), "Document");
        // But a dotfile with a real extension still has one.
        assert_eq!(label_for(".env.json"), "JSON Text");
    }

    #[test]
    fn matching_ignores_case_and_uses_the_last_extension() {
        assert_eq!(label_for("Photo.JPG"), "JPEG Image");
        assert_eq!(label_for("backup.tar.gz"), "Gzip Archive");
    }

    #[test]
    fn a_name_that_is_entirely_a_known_word_is_recognised() {
        // `Makefile` and `Dockerfile` have no extension but are not mysteries.
        assert_eq!(label_for("Makefile"), "Makefile");
        assert_eq!(label_for("Dockerfile"), "Dockerfile");
    }

    #[test]
    fn directories_and_links_are_labelled_by_what_they_are() {
        assert_eq!(classify_entry("Series", EntryKind::Dir).label, "Folder");
        assert_eq!(
            classify_entry("Series", EntryKind::Dir).category,
            Category::Folder
        );
        assert_eq!(classify_entry("latest", EntryKind::Symlink).label, "Alias");
    }

    #[test]
    fn a_dotfile_reads_as_hidden_but_keeps_the_category_of_its_contents() {
        let kind = classify_entry(".env.json", EntryKind::File);
        // The column says "Hidden", per the design...
        assert_eq!(kind.label, "Hidden");
        // ...but Quick Look can still show it, because it is still text.
        assert_eq!(kind.category, Category::Text);
        assert!(kind.category.previewable());
    }

    #[test]
    fn a_folder_is_promised_as_a_folder_and_not_as_bytes() {
        // The one mapping the drag out to Finder cannot get wrong: a folder
        // offered as `public.data` is a drag Finder accepts and then writes a
        // *file* for, so the tree never appears.
        assert_eq!(
            classify_entry("Films", EntryKind::Dir).category.uti(),
            "public.folder"
        );
        assert_eq!(classify("Arrival.mkv").category.uti(), "public.movie");
        assert_eq!(classify("notes.txt").category.uti(), "public.plain-text");
        // An extension nothing in the table knows is still a promise; it is
        // just an unspecific one.
        assert_eq!(classify("archive.xyz").category.uti(), "public.data");
    }

    #[test]
    fn every_promised_type_is_a_reverse_dns_identifier_apple_could_know() {
        // A misspelt or invented UTI is not a cosmetic error: it makes a drag
        // that no application will accept, and nothing else in the app reads
        // this string, so nothing else would notice. The shape is all a test
        // can check — that it is a dotted identifier under a real authority —
        // but it catches the empty string and the bare word, which are the two
        // ways a hand-written table like this actually rots.
        for category in [
            Category::Folder,
            Category::Alias,
            Category::Video,
            Category::Audio,
            Category::Image,
            Category::Pdf,
            Category::Text,
            Category::Code,
            Category::Document,
            Category::Archive,
            Category::Disk,
            Category::Application,
            Category::Font,
            Category::Data,
            Category::Other,
        ] {
            let uti = category.uti();
            assert!(
                uti.starts_with("public.") || uti.starts_with("com."),
                "{category:?} is promised as {uti}"
            );
            assert!(
                uti.split('.').all(|part| !part.is_empty()),
                "{category:?} is promised as {uti}"
            );
        }
    }

    #[test]
    fn previewable_covers_what_quick_look_can_actually_open() {
        assert!(Category::Image.previewable());
        assert!(Category::Video.previewable());
        assert!(Category::Code.previewable());
        assert!(!Category::Archive.previewable());
        assert!(!Category::Folder.previewable());
        assert!(!Category::Other.previewable());
    }
}
