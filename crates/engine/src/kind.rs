//! Extension to human-readable kind, for the Kind column.
//!
//! Deliberately a table over names rather than content sniffing: a listing must
//! not read any file, and the design's Kind column is a label, not a guarantee.

/// The Kind label for a name, e.g. `"Matroska Video"`.
///
/// Directories, symlinks and hidden files are labelled by the caller, which
/// knows what the entry is; this answers only for regular files.
#[must_use]
pub fn label_for(name: &str) -> String {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "mkv" => "Matroska Video",
        "mp4" | "m4v" => "MPEG-4 Video",
        "mov" => "QuickTime Movie",
        "avi" => "AVI Video",
        "webm" => "WebM Video",
        "srt" => "SubRip Text",
        "vtt" => "WebVTT Text",
        "mp3" => "MP3 Audio",
        "flac" => "FLAC Audio",
        "wav" => "WAV Audio",
        "m4a" | "aac" => "AAC Audio",
        "jpg" | "jpeg" => "JPEG Image",
        "png" => "PNG Image",
        "gif" => "GIF Image",
        "webp" => "WebP Image",
        "heic" => "HEIF Image",
        "tif" | "tiff" => "TIFF Image",
        "svg" => "SVG Image",
        "pdf" => "PDF Document",
        "txt" | "text" => "Plain Text",
        "md" | "markdown" => "Markdown Text",
        "rtf" => "Rich Text",
        "json" => "JSON Text",
        "xml" => "XML Text",
        "yaml" | "yml" => "YAML Text",
        "toml" => "TOML Text",
        "csv" => "CSV Text",
        "html" | "htm" => "HTML Document",
        "css" => "CSS Text",
        "js" | "mjs" => "JavaScript Source",
        "ts" | "tsx" => "TypeScript Source",
        "rs" => "Rust Source",
        "py" => "Python Source",
        "go" => "Go Source",
        "c" | "h" => "C Source",
        "sh" | "bash" => "Shell Script",
        "zip" => "ZIP Archive",
        "tar" => "TAR Archive",
        "gz" | "tgz" => "Gzip Archive",
        "zst" => "Zstandard Archive",
        "bz2" => "Bzip2 Archive",
        "7z" => "7-Zip Archive",
        "dmg" | "img" | "qcow2" | "vmdk" => "Disk Image",
        "iso" => "Disc Image",
        "app" => "Application",
        "log" => "Log File",
        "conf" | "cfg" | "ini" => "Configuration",
        "" => "Document",
        other => return format!("{} File", other.to_ascii_uppercase()),
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::label_for;

    #[test]
    fn known_extensions_get_the_designs_wording() {
        // These two are the labels the mock in the handoff shows, so they are
        // the ones a pixel comparison checks.
        assert_eq!(label_for("Arrival.2016.mkv"), "Matroska Video");
        assert_eq!(label_for("Arrival.2016.srt"), "SubRip Text");
    }

    #[test]
    fn an_unknown_extension_is_named_rather_than_called_unknown() {
        assert_eq!(label_for("archive.xyz"), "XYZ File");
        // No extension at all is not the same as an unrecognised one.
        assert_eq!(label_for("README"), "Document");
    }

    #[test]
    fn matching_ignores_case_and_uses_the_last_extension() {
        assert_eq!(label_for("Photo.JPG"), "JPEG Image");
        assert_eq!(label_for("backup.tar.gz"), "Gzip Archive");
    }
}
