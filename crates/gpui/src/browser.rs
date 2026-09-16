//! Pure file-browser state for the GPUI shell.
//!
//! This module is deliberately independent of GPUI.  A model can own a
//! [`BrowserState`] and call these methods from event handlers, while the
//! shell remains responsible for listing a directory and painting the result.
//! The vocabulary used by a listing is the engine's public vocabulary, so a
//! row cannot acquire a subtly different meaning at the UI boundary.
//!
//! The functions here mirror the pure TypeScript state modules in
//! `src/state/`.  Paths are share-relative, slash-separated, and use the empty
//! string for the share root.  Selection is keyed by path rather than row
//! index: sorting, filtering, and a refresh therefore cannot make a selected
//! row point at a different file.

use std::cmp::Ordering;
use std::collections::HashSet;

#[allow(unused_imports)]
pub use excalibur_engine::backend::{
    Access, Attrs, Capacity, Caps, ConnectionSpec, Entry, EntryKind, Listing, MountInfo,
    MountStatus, Protocol,
};
#[allow(unused_imports)]
pub use excalibur_engine::kind::Category;

/// The list column used for ordering rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortColumn {
    #[default]
    Name,
    Size,
    Added,
    Modified,
    Kind,
}

/// Row presentation chosen by the View menu. This is independent of sorting
/// and selection: switching from columns to the mono grid must not change
/// which rows are selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowLayout {
    #[default]
    Columns,
    NameLed,
    Mono,
    Icons,
    ColumnView,
}

/// Alias matching the design vocabulary where a layout is called a view.
pub type View = RowLayout;

/// A column and its direction.  `ascending` is the direction of the column's
/// value; folders-first remains in force in either direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: SortColumn,
    pub ascending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Sort {
    /// The default is an ascending natural name sort.
    pub const DEFAULT: Self = Self {
        column: SortColumn::Name,
        ascending: true,
    };

    /// Apply a header click. Names and kinds start ascending; numeric columns
    /// start descending because a first click normally means newest/largest.
    #[must_use]
    pub fn toggle(self, column: SortColumn) -> Self {
        if self.column == column {
            Self {
                column,
                ascending: !self.ascending,
            }
        } else {
            Self {
                column,
                ascending: matches!(column, SortColumn::Name | SortColumn::Kind),
            }
        }
    }
}

impl Sort {
    /// Alias useful to callers porting the TypeScript `toggle` helper.
    #[must_use]
    pub fn clicked(self, column: SortColumn) -> Self {
        self.toggle(column)
    }
}

/// Unicode folding used by filtering, type-ahead and tie-breaking.
///
/// Rust's standard library intentionally does not provide Unicode
/// decomposition.  This covers combining marks (which are already separate
/// chars) and the Latin letters most likely to occur in names.  It is not a
/// claim of complete transliteration; it is the same practical table as the
/// browser implementation.
#[must_use]
pub fn fold(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for ch in text.chars().flat_map(char::to_lowercase) {
        let replacement = match ch {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
            'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
            'ď' | 'ð' | 'đ' => "d",
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'ĥ' | 'ħ' => "h",
            'ĵ' => "j",
            'ķ' => "k",
            'ĺ' | 'ľ' | 'ļ' | 'ŀ' | 'ł' => "l",
            'ñ' | 'ń' | 'ň' | 'ņ' | 'ŉ' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'ŕ' | 'ř' | 'ŗ' => "r",
            'ś' | 'š' | 'ş' | 'ŝ' | 'ș' => "s",
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'ť' | 'ț' | 'ŧ' => "t",
            'ŵ' => "w",
            'ý' | 'ÿ' => "y",
            'ź' | 'ž' | 'ż' => "z",
            'æ' => "ae",
            'œ' => "oe",
            'ß' => "ss",
            'þ' => "th",
            // Combining marks are discarded.  This includes the decomposed
            // accents produced by callers before this function is reached.
            '\u{300}'..='\u{36f}' => "",
            _ => {
                output.push(ch);
                continue;
            }
        };
        output.push_str(replacement);
    }
    output
}

/// Whether `name` contains the case/diacritic-insensitive `needle`.
#[must_use]
pub fn matches(name: &str, needle: &str) -> bool {
    needle.is_empty() || fold(name).contains(&fold(needle))
}

/// Filter rows without mutating the source listing.
#[must_use]
pub fn filter_entries<'a>(entries: &'a [Entry], needle: &str) -> Vec<&'a Entry> {
    if needle.trim().is_empty() {
        return entries.iter().collect();
    }
    entries
        .iter()
        .filter(|entry| matches(&entry.name, needle))
        .collect()
}

/// Filter rows into owned entries, for callers that need a standalone
/// projection (for example, a background derivation).
#[must_use]
pub fn filtered_entries(entries: &[Entry], needle: &str) -> Vec<Entry> {
    filter_entries(entries, needle)
        .into_iter()
        .cloned()
        .collect()
}

fn natural_cmp(left: &str, right: &str) -> Ordering {
    let left = fold(left);
    let right = fold(right);
    let mut a = left.chars().peekable();
    let mut b = right.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut ax = String::new();
                let mut by = String::new();
                while a.peek().is_some_and(char::is_ascii_digit) {
                    ax.push(a.next().unwrap_or_default());
                }
                while b.peek().is_some_and(char::is_ascii_digit) {
                    by.push(b.next().unwrap_or_default());
                }
                let at = ax.trim_start_matches('0');
                let bt = by.trim_start_matches('0');
                let order = at.len().cmp(&bt.len()).then_with(|| at.cmp(bt));
                if order != Ordering::Equal {
                    return order;
                }
                // `01` and `1` compare equal numerically; fewer leading zeroes
                // gives a deterministic answer before the final path tie-break.
                let order = ax.len().cmp(&by.len());
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.cmp(&y);
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn sort_value_cmp(left: &Entry, right: &Entry, column: SortColumn) -> Ordering {
    match column {
        SortColumn::Name => natural_cmp(&left.name, &right.name),
        SortColumn::Kind => natural_cmp(&left.kind_label, &right.kind_label),
        SortColumn::Size => left.attrs.size.cmp(&right.attrs.size),
        SortColumn::Modified => left
            .attrs
            .modified_ms
            .unwrap_or(i64::MIN)
            .cmp(&right.attrs.modified_ms.unwrap_or(i64::MIN)),
        SortColumn::Added => left
            .attrs
            .added_ms
            .unwrap_or(i64::MIN)
            .cmp(&right.attrs.added_ms.unwrap_or(i64::MIN)),
    }
}

/// Sort a listing.  Ties are broken by natural name order for deterministic
/// refreshes; the input vector is not modified.
#[must_use]
pub fn sort_entries(entries: &[Entry], sort: Sort, folders_first: bool) -> Vec<Entry> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|left, right| {
        if folders_first
            && (left.attrs.kind == EntryKind::Dir) != (right.attrs.kind == EntryKind::Dir)
        {
            return if left.attrs.kind == EntryKind::Dir {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let order = sort_value_cmp(left, right, sort.column);
        let order = if sort.ascending {
            order
        } else {
            order.reverse()
        };
        if order == Ordering::Equal {
            natural_cmp(&left.name, &right.name)
        } else {
            order
        }
    });
    sorted
}

/// Selection state. Paths, rather than indexes, survive sort/filter/refresh.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub paths: HashSet<String>,
    /// The fixed end used by Shift-click and Shift-arrow.
    pub anchor: Option<String>,
}

/// Modifier keys used by a click or rubber-band selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub toggle: bool,
}

/// Inclusive paths between two rows.  If either endpoint is hidden, only the
/// explicitly clicked endpoint is returned, matching browser behaviour after
/// a filter or sort removed the anchor.
#[must_use]
pub fn between(order: &[String], from: &str, to: &str) -> Vec<String> {
    let Some(start) = order.iter().position(|path| path == from) else {
        return vec![to.to_owned()];
    };
    let Some(end) = order.iter().position(|path| path == to) else {
        return vec![to.to_owned()];
    };
    order[start.min(end)..=start.max(end)].to_vec()
}

/// Apply a row click to selection state.
#[must_use]
pub fn click(current: &Selection, order: &[String], path: &str, modifiers: Modifiers) -> Selection {
    if modifiers.toggle {
        let mut paths = current.paths.clone();
        if !paths.remove(path) {
            paths.insert(path.to_owned());
        }
        return Selection {
            paths,
            anchor: Some(path.to_owned()),
        };
    }
    if modifiers.shift {
        if let Some(anchor) = current.anchor.as_deref() {
            return Selection {
                paths: between(order, anchor, path).into_iter().collect(),
                anchor: Some(anchor.to_owned()),
            };
        }
    }
    Selection {
        paths: [path.to_owned()].into_iter().collect(),
        anchor: Some(path.to_owned()),
    }
}

/// Replace or add a rubber-band's covered rows.
#[must_use]
pub fn band(current: &Selection, covered: &[String], additive: bool) -> Selection {
    let mut paths = if additive {
        current.paths.clone()
    } else {
        HashSet::new()
    };
    paths.extend(covered.iter().cloned());
    Selection {
        paths,
        anchor: current.anchor.clone().or_else(|| covered.first().cloned()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub selection: Selection,
    pub reveal: Option<usize>,
}

fn focus_index(order: &[String], selection: &Selection) -> Option<usize> {
    if selection.paths.is_empty() {
        return None;
    }
    if let Some(anchor) = &selection.anchor {
        if let Some(index) = order.iter().position(|path| path == anchor) {
            return Some(index);
        }
    }
    order
        .iter()
        .enumerate()
        .filter(|(_, path)| selection.paths.contains(*path))
        .map(|(index, _)| index)
        .next_back()
}

/// The selected row furthest from the anchor. Ties choose the later row.
#[must_use]
pub fn far_index(order: &[String], selection: &Selection) -> Option<usize> {
    let anchor = focus_index(order, selection)?;
    order
        .iter()
        .enumerate()
        .filter(|(_, path)| selection.paths.contains(*path))
        .max_by_key(|(index, _)| (index.abs_diff(anchor), *index))
        .map(|(index, _)| index)
}

fn move_to(order: &[String], index: isize) -> Move {
    if order.is_empty() {
        return Move {
            selection: Selection::default(),
            reveal: None,
        };
    }
    let index =
        usize::try_from(index.clamp(0, order.len().try_into().unwrap_or(isize::MAX))).unwrap_or(0);
    let path = order[index.min(order.len() - 1)].clone();
    Move {
        selection: Selection {
            paths: [path.clone()].into_iter().collect(),
            anchor: Some(path),
        },
        reveal: Some(index.min(order.len() - 1)),
    }
}

/// Select a row by index, clamped to the list.
#[must_use]
pub fn move_to_index(order: &[String], index: isize) -> Move {
    move_to(order, index)
}

/// Move by rows. An empty selection enters at the first/last row.
#[must_use]
pub fn step(selection: &Selection, order: &[String], delta: isize) -> Move {
    if order.is_empty() {
        return Move {
            selection: selection.clone(),
            reveal: None,
        };
    }
    match focus_index(order, selection) {
        Some(index) => move_to(order, index.try_into().unwrap_or(isize::MAX) + delta),
        None => move_to(
            order,
            if delta > 0 {
                0
            } else {
                order.len().try_into().unwrap_or(isize::MAX) - 1
            },
        ),
    }
}

/// Extend a range to an index while retaining its anchor.
#[must_use]
pub fn extend_to(selection: &Selection, order: &[String], index: isize) -> Move {
    if order.is_empty() {
        return Move {
            selection: selection.clone(),
            reveal: None,
        };
    }
    let index =
        usize::try_from(index.clamp(0, order.len().try_into().unwrap_or(isize::MAX))).unwrap_or(0);
    let path = order[index.min(order.len() - 1)].clone();
    let Some(anchor_index) = focus_index(order, selection) else {
        return move_to(order, index.try_into().unwrap_or(isize::MAX));
    };
    let anchor = order[anchor_index].clone();
    Move {
        selection: Selection {
            paths: between(order, &anchor, &path).into_iter().collect(),
            anchor: Some(anchor),
        },
        reveal: Some(index),
    }
}

/// Extend the far end by rows.
#[must_use]
pub fn extend_by(selection: &Selection, order: &[String], delta: isize) -> Move {
    if order.is_empty() {
        return Move {
            selection: selection.clone(),
            reveal: None,
        };
    }
    match far_index(order, selection) {
        Some(index) => extend_to(
            selection,
            order,
            index.try_into().unwrap_or(isize::MAX) + delta,
        ),
        None => move_to(
            order,
            if delta > 0 {
                0
            } else {
                order.len().try_into().unwrap_or(isize::MAX) - 1
            },
        ),
    }
}

/// Find a type-ahead match. `origin` is used when the buffer is growing;
/// repeated copies of one character cycle from `current`.
#[must_use]
pub fn type_ahead(
    order: &[String],
    names: &[String],
    buffer: &str,
    current: Option<&str>,
    origin: Option<&str>,
) -> Option<String> {
    if buffer.is_empty() || order.is_empty() {
        return None;
    }
    let first = buffer.chars().next()?;
    let repeated = buffer.chars().all(|ch| ch == first);
    let needle = if repeated {
        fold(&first.to_string())
    } else {
        fold(buffer)
    };
    let anchor = if repeated { current } else { origin };
    let start = match anchor {
        Some(anchor) => order
            .iter()
            .position(|path| path == anchor)
            .unwrap_or(order.len() - 1),
        None => order.len() - 1,
    };
    for offset in 1..=order.len() {
        let index = (start + offset) % order.len();
        if names
            .get(index)
            .is_some_and(|name| fold(name).starts_with(&needle))
        {
            return order.get(index).cloned();
        }
    }
    None
}

/// A share-relative browsing history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
    pub stack: Vec<String>,
    pub index: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::initial("")
    }
}

impl History {
    #[must_use]
    pub fn initial(path: impl Into<String>) -> Self {
        Self {
            stack: vec![path.into()],
            index: 0,
        }
    }
    #[must_use]
    pub fn current(&self) -> &str {
        self.stack.get(self.index).map_or("", String::as_str)
    }
    #[must_use]
    pub const fn can_back(&self) -> bool {
        self.index > 0
    }
    #[must_use]
    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.stack.len()
    }
    #[must_use]
    pub fn go(&self, path: impl Into<String>) -> Self {
        let path = path.into();
        if self.current() == path {
            return self.clone();
        }
        let mut stack = self.stack[..=self.index].to_vec();
        stack.push(path);
        Self {
            index: stack.len() - 1,
            stack,
        }
    }
    #[must_use]
    pub fn back(&self) -> Self {
        if self.can_back() {
            Self {
                index: self.index - 1,
                ..self.clone()
            }
        } else {
            self.clone()
        }
    }
    #[must_use]
    pub fn forward(&self) -> Self {
        if self.can_forward() {
            Self {
                index: self.index + 1,
                ..self.clone()
            }
        } else {
            self.clone()
        }
    }
}

#[must_use]
pub fn parent(path: &str) -> String {
    path.rsplit_once('/')
        .map_or_else(String::new, |(parent, _)| parent.to_owned())
}

/// Functional wrappers retained for callers that prefer the TypeScript
/// module's shape over methods on [`History`].
#[must_use]
pub fn initial(path: impl Into<String>) -> History {
    History::initial(path)
}
#[must_use]
pub fn current(history: &History) -> &str {
    history.current()
}
#[must_use]
pub const fn can_back(history: &History) -> bool {
    history.can_back()
}
#[must_use]
pub fn can_forward(history: &History) -> bool {
    history.can_forward()
}
#[must_use]
pub fn go(history: &History, path: impl Into<String>) -> History {
    history.go(path)
}
#[must_use]
pub fn back(history: &History) -> History {
    history.back()
}
#[must_use]
pub fn forward(history: &History) -> History {
    history.forward()
}
#[must_use]
pub const fn can_ascend(path: &str) -> bool {
    !path.is_empty()
}
#[must_use]
pub fn segments(path: &str) -> Vec<&str> {
    if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    }
}
#[must_use]
pub fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}/{name}")
    }
}
#[must_use]
pub fn ancestor(path: &str, count: usize) -> String {
    segments(path)
        .into_iter()
        .take(count)
        .collect::<Vec<_>>()
        .join("/")
}

/// The path field's three outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    Navigate { path: String },
    Connect(ConnectionDraft),
    Invalid { reason: String },
}

/// Credentials are intentionally absent. The shell resolves a draft through
/// its secure keychain path; this parser must never create a password value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionDraft {
    pub protocol: Protocol,
    pub host: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub share: String,
}

impl ConnectionDraft {
    /// Convert the safe, user-entered part of a location into the engine's
    /// connection contract. The caller supplies the stable bookmark identity
    /// and mount preference; no secret is represented by either type.
    #[must_use]
    pub fn into_spec(self, id: String, name: String, mount_preference: Access) -> ConnectionSpec {
        ConnectionSpec {
            id,
            name,
            protocol: self.protocol,
            host: self.host,
            port: self.port,
            share: self.share,
            username: self.username,
            domain: None,
            mount_preference,
        }
    }
}

/// Parse a path or `scheme://` location. `home` is only used for local `~`.
#[must_use]
pub fn parse_location(input: &str, home: Option<&str>) -> Option<Location> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    let Some((scheme, rest)) = text.split_once("://") else {
        return Some(Location::Navigate {
            path: text.to_owned(),
        });
    };
    let protocol = match scheme.to_ascii_lowercase().as_str() {
        "smb" => Protocol::Smb,
        "nfs" => Protocol::Nfs,
        "sftp" => Protocol::Sftp,
        "xsync" => Protocol::Xsync,
        "file" => Protocol::Local,
        _ => {
            return Some(Location::Invalid {
                reason: format!("{scheme}:// is not a protocol this app speaks"),
            });
        }
    };
    if protocol == Protocol::Local {
        let path = rest
            .strip_prefix("localhost/")
            .map_or_else(|| rest.to_owned(), |path| format!("/{path}"));
        let path = if let Some(home) = home {
            if path == "~" {
                home.to_owned()
            } else if let Some(rest) = path.strip_prefix("~/") {
                format!("{home}/{rest}")
            } else {
                path
            }
        } else {
            path
        };
        return Some(if path.starts_with('/') {
            Location::Connect(ConnectionDraft {
                protocol,
                host: String::new(),
                port: None,
                username: None,
                share: path,
            })
        } else {
            Location::Invalid {
                reason: "a file:// location needs an absolute path".to_owned(),
            }
        });
    }
    let authority = rest.split('/').next().unwrap_or("");
    let share = rest
        .strip_prefix(authority)
        .unwrap_or("")
        .trim_start_matches('/')
        .to_owned();
    let (username, host_port): (Option<String>, &str) = authority
        .rsplit_once('@')
        .map_or((None, authority), |(user, host)| {
            (Some(user.to_owned()), host)
        });
    let (host, port_text) = host_port
        .rsplit_once(':')
        .map_or((host_port, ""), |(host, port)| (host, port));
    if host.is_empty() {
        return Some(Location::Invalid {
            reason: format!("{scheme}:// needs a host"),
        });
    }
    let port = match port_text {
        "" => None,
        text if text.chars().all(|ch| ch.is_ascii_digit()) => match text.parse() {
            Ok(port) => Some(port),
            Err(_) => {
                return Some(Location::Invalid {
                    reason: format!("{text} is not a port"),
                });
            }
        },
        text => {
            return Some(Location::Invalid {
                reason: format!("{text} is not a port"),
            });
        }
    };
    Some(Location::Connect(ConnectionDraft {
        protocol,
        host: host.to_owned(),
        port,
        username,
        share,
    }))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "human-readable decimal formatting accepts f64 precision"
)]
fn unit_size(bytes: u64, terse: bool) -> String {
    if bytes < 1000 {
        return if terse {
            bytes.to_string()
        } else {
            format!("{bytes} B")
        };
    }
    let units = if terse {
        ["K", "M", "G", "T", "P"]
    } else {
        ["KB", "MB", "GB", "TB", "PB"]
    };
    let mut value = bytes as f64 / 1000.0;
    let mut index = 0;
    while value >= 1000.0 && index < units.len() - 1 {
        value /= 1000.0;
        index += 1;
    }
    let rendered = if value >= 100.0 || (!terse && value.fract() == 0.0) {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    };
    if terse {
        format!("{rendered}{}", units[index])
    } else {
        format!("{rendered} {}", units[index])
    }
}
#[must_use]
pub fn format_size(bytes: u64) -> String {
    unit_size(bytes, false)
}
#[must_use]
pub fn format_size_terse(bytes: u64) -> String {
    unit_size(bytes, true)
}
#[must_use]
pub fn format_capacity(free: u64, total: u64) -> String {
    format!("{} free of {}", format_size(free), format_size(total))
}
#[must_use]
pub fn format_counts(
    items: usize,
    selected: usize,
    selected_bytes: u64,
    total: Option<usize>,
) -> String {
    let item = total.filter(|total| *total != items).map_or_else(
        || format!("{items} item{}", if items == 1 { "" } else { "s" }),
        |total| {
            format!(
                "{items} of {total} item{}",
                if total == 1 { "" } else { "s" }
            )
        },
    );
    if selected == 0 {
        item
    } else {
        format!(
            "{item} · {selected} selected · {}",
            format_size(selected_bytes)
        )
    }
}
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the clamped display duration is converted to whole seconds"
)]
pub fn format_duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0).floor() as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        let minutes = seconds / 60;
        if minutes < 60 {
            format!("{minutes}m {:02}s", seconds % 60)
        } else {
            format!("{}h {:02}m", minutes / 60, minutes % 60)
        }
    }
}
#[must_use]
pub fn format_rate(bytes_per_second: u64) -> String {
    format!("{}/s", format_size(bytes_per_second))
}
#[must_use]
pub fn extension_of(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty() && !extension.is_empty()).then(|| extension.to_ascii_lowercase())
}
#[must_use]
pub fn extension_label(name: &str) -> String {
    extension_of(name).unwrap_or_else(|| "—".to_owned())
}
#[must_use]
pub fn format_mount_labels(protocol: &str, security: &str) -> String {
    if protocol == security {
        protocol.to_owned()
    } else {
        format!("{protocol} · {security}")
    }
}

/// Calendar fields for a Unix timestamp, in UTC.
///
/// The browser implementation uses the window's local timezone. Keeping this
/// helper timezone-neutral avoids adding a date crate to the native shell;
/// callers that need local presentation can convert the timestamp before
/// calling it. UTC is also deterministic for snapshots and tests.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "euclidean calendar arithmetic bounds these display fields"
)]
fn unix_calendar(ms: i64) -> Option<(i64, u32, u32, u32, u32)> {
    let days = ms.div_euclid(86_400_000);
    let day_ms = ms.rem_euclid(86_400_000);
    // Civil-from-days, H. Hinnant (public-domain algorithm), with the Unix
    // epoch as day zero.
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    let hour = (day_ms / 3_600_000) as u32;
    let minute = ((day_ms % 3_600_000) / 60_000) as u32;
    u32::try_from(month)
        .ok()
        .map(|month| (year, month, day as u32, hour, minute))
}

/// The full Modified/Added column format. Unknown timestamps render as an
/// em dash instead of pretending that the Unix epoch is meaningful.
#[must_use]
pub fn format_modified(ms: Option<i64>) -> String {
    let Some((year, month, day, hour, minute)) = ms.and_then(unix_calendar) else {
        return "—".to_owned();
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{day} {} {year}, {hour:02}:{minute:02}",
        months[(month - 1) as usize]
    )
}

/// Compact row format without a year.
#[must_use]
pub fn format_modified_short(ms: Option<i64>) -> String {
    let full = format_modified(ms);
    if full == "—" {
        return full;
    }
    let mut parts = full.splitn(3, ' ');
    let day = parts.next().unwrap_or("");
    let month = parts.next().unwrap_or("");
    let time = parts
        .next()
        .unwrap_or("")
        .split(',')
        .nth(1)
        .map_or("", str::trim);
    format!("{day} {month} {time}")
}

/// Fixed-width mono-grid date format (MM-DD HH:MM).
#[must_use]
pub fn format_modified_grid(ms: Option<i64>) -> String {
    let Some((_, month, day, hour, minute)) = ms.and_then(unix_calendar) else {
        return "—".to_owned();
    };
    format!("{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// A write/read affordance and the one gate used by every shell surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Affordance {
    QuickLook,
    Download,
    Edit,
    Upload,
    Rename,
    Move,
    Delete,
    NewFolder,
    DropTarget,
    RemountRw,
    Properties,
}
pub const READ_AFFORDANCES: &[Affordance] = &[
    Affordance::QuickLook,
    Affordance::Download,
    Affordance::Properties,
];
pub const WRITE_AFFORDANCES: &[Affordance] = &[
    Affordance::Edit,
    Affordance::Upload,
    Affordance::Rename,
    Affordance::Move,
    Affordance::Delete,
    Affordance::NewFolder,
    Affordance::DropTarget,
];
pub const RECOVERY_AFFORDANCES: &[Affordance] = &[Affordance::RemountRw];
pub const AFFORDANCES: &[Affordance] = &[
    Affordance::QuickLook,
    Affordance::Download,
    Affordance::Properties,
    Affordance::Edit,
    Affordance::Upload,
    Affordance::Rename,
    Affordance::Move,
    Affordance::Delete,
    Affordance::NewFolder,
    Affordance::DropTarget,
    Affordance::RemountRw,
];
/// Shared reason used when no backend is mounted.
pub const NO_SHARE_REASON: &str = "No share connected";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PermissionContext<'a> {
    pub writable: bool,
    pub reason: &'a str,
    pub has_selection: bool,
    pub editable: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    pub disabled: bool,
    pub reason: Option<String>,
}

/// Evaluate an affordance. A read-only reason takes precedence over selection;
/// that is the problem a click cannot fix.
#[must_use]
pub fn gate(affordance: Affordance, context: PermissionContext<'_>) -> Gate {
    if affordance == Affordance::RemountRw {
        return if context.writable {
            Gate {
                disabled: true,
                reason: Some("Already mounted read-write".to_owned()),
            }
        } else {
            Gate {
                disabled: false,
                reason: None,
            }
        };
    }
    let write = matches!(
        affordance,
        Affordance::Edit
            | Affordance::Upload
            | Affordance::Rename
            | Affordance::Move
            | Affordance::Delete
            | Affordance::NewFolder
            | Affordance::DropTarget
    );
    if write && !context.writable {
        return Gate {
            disabled: true,
            reason: Some(context.reason.to_owned()),
        };
    }
    let needs_selection = matches!(
        affordance,
        Affordance::QuickLook
            | Affordance::Download
            | Affordance::Edit
            | Affordance::Rename
            | Affordance::Move
            | Affordance::Delete
            | Affordance::Properties
    );
    if needs_selection && !context.has_selection {
        return Gate {
            disabled: true,
            reason: Some("Nothing selected".to_owned()),
        };
    }
    if affordance == Affordance::Edit && context.editable == Some(false) {
        return Gate {
            disabled: true,
            reason: Some("Only text files can be edited here".to_owned()),
        };
    }
    Gate {
        disabled: false,
        reason: None,
    }
}
#[must_use]
pub fn accepts_drop(context: PermissionContext<'_>) -> bool {
    !gate(Affordance::DropTarget, context).disabled
}

/// Copy for the Quick Look footer, which explains the recovery action on a
/// read-only mount instead of merely repeating that editing is disabled.
#[must_use]
pub fn quick_look_footer(context: PermissionContext<'_>) -> &'static str {
    if context.writable {
        "Edit opens this in your default editor"
    } else {
        "Read-only — download a copy to edit"
    }
}

/// The state a model needs for one pane. Listing I/O and asynchronous stale
/// result handling stay in the model; this type only applies committed state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserState {
    history: History,
    entries: Vec<Entry>,
    mount: Option<MountInfo>,
    sort: Sort,
    filter: String,
    folders_first: bool,
    show_hidden: bool,
    layout: RowLayout,
    selection: Selection,
    generation: u64,
}

impl BrowserState {
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            history: History::initial(path),
            entries: Vec::new(),
            mount: None,
            sort: Sort::DEFAULT,
            filter: String::new(),
            folders_first: true,
            show_hidden: true,
            layout: RowLayout::Columns,
            selection: Selection::default(),
            generation: 0,
        }
    }
    #[must_use]
    pub fn path(&self) -> &str {
        self.history.current()
    }
    #[must_use]
    pub const fn history(&self) -> &History {
        &self.history
    }
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        let filtered = filter_entries(&self.entries, &self.filter)
            .into_iter()
            .filter(|entry| self.show_hidden || !entry.hidden)
            .collect::<Vec<_>>();
        sort_entries(
            &filtered.into_iter().cloned().collect::<Vec<_>>(),
            self.sort,
            self.folders_first,
        )
    }
    #[must_use]
    pub fn raw_entries(&self) -> &[Entry] {
        &self.entries
    }
    #[must_use]
    pub const fn mount(&self) -> Option<&MountInfo> {
        self.mount.as_ref()
    }
    #[must_use]
    pub const fn sort(&self) -> Sort {
        self.sort
    }
    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }
    #[must_use]
    pub const fn selection(&self) -> &Selection {
        &self.selection
    }
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    pub fn set_listing(&mut self, listing: Listing) {
        self.entries = listing.entries;
        self.mount = Some(listing.mount);
        self.generation = self.generation.wrapping_add(1);
        self.selection
            .paths
            .retain(|path| self.entries.iter().any(|entry| &entry.path == path));
    }
    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
    }
    pub fn set_folders_first(&mut self, folders_first: bool) {
        self.folders_first = folders_first;
    }
    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        self.show_hidden = show_hidden;
    }
    #[must_use]
    pub const fn show_hidden(&self) -> bool {
        self.show_hidden
    }
    #[must_use]
    pub const fn folders_first(&self) -> bool {
        self.folders_first
    }
    pub fn set_layout(&mut self, layout: RowLayout) {
        self.layout = layout;
    }
    #[must_use]
    pub const fn layout(&self) -> RowLayout {
        self.layout
    }
    pub fn toggle_sort(&mut self, column: SortColumn) {
        self.sort = self.sort.toggle(column);
    }
    pub fn navigate(&mut self, path: impl Into<String>) {
        self.history = self.history.go(path);
        self.entries.clear();
        self.selection = Selection::default();
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn back(&mut self) {
        let next = self.history.back();
        if next != self.history {
            self.history = next;
            self.entries.clear();
            self.selection = Selection::default();
            self.generation = self.generation.wrapping_add(1);
        }
    }
    pub fn forward(&mut self) {
        let next = self.history.forward();
        if next != self.history {
            self.history = next;
            self.entries.clear();
            self.selection = Selection::default();
            self.generation = self.generation.wrapping_add(1);
        }
    }
    pub fn click(&mut self, path: &str, modifiers: Modifiers) {
        let order = self
            .entries()
            .into_iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        self.selection = click(&self.selection, &order, path, modifiers);
    }
    pub fn step(&mut self, delta: isize) -> Option<usize> {
        let order = self
            .entries()
            .into_iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let moved = step(&self.selection, &order, delta);
        let reveal = moved.reveal;
        self.selection = moved.selection;
        reveal
    }
    pub fn extend_by(&mut self, delta: isize) -> Option<usize> {
        let order = self
            .entries()
            .into_iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let moved = extend_by(&self.selection, &order, delta);
        let reveal = moved.reveal;
        self.selection = moved.selection;
        reveal
    }
}

#[cfg(test)]
mod tests {
    use super::{
        accepts_drop, click, extension_of, format_modified, format_modified_grid,
        format_modified_short, format_size, gate, matches, parse_location, sort_entries,
        type_ahead, Affordance, Attrs, BrowserState, Category, ConnectionDraft, Entry, EntryKind,
        Gate, History, Location, Modifiers, PermissionContext, Protocol, RowLayout, Selection,
        Sort, SortColumn,
    };

    fn entry(name: &str, kind: EntryKind, size: u64) -> Entry {
        Entry {
            name: name.to_owned(),
            path: name.to_owned(),
            attrs: Attrs {
                kind,
                size,
                modified_ms: None,
                added_ms: None,
                mode: None,
                link_target: None,
                change_cookie: None,
            },
            kind_label: if kind == EntryKind::Dir {
                "Folder"
            } else {
                "Document"
            }
            .to_owned(),
            kind_category: if kind == EntryKind::Dir {
                Category::Folder
            } else {
                Category::Other
            },
            hidden: name.starts_with('.'),
        }
    }

    #[test]
    fn sorting_is_natural_and_keeps_folders_first_when_reversed() {
        let rows = vec![
            entry("File 10", EntryKind::File, 1),
            entry("File 2", EntryKind::File, 2),
            entry("Dir", EntryKind::Dir, 0),
        ];
        let rows = sort_entries(
            &rows,
            Sort {
                column: SortColumn::Name,
                ascending: false,
            },
            true,
        );
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            ["Dir", "File 10", "File 2"]
        );
    }

    #[test]
    fn presentation_preferences_filter_order_and_keep_selection_path_keyed() {
        let mut state = BrowserState::new("");
        state.entries = vec![
            entry(".hidden", EntryKind::File, 1),
            entry("a-visible", EntryKind::File, 2),
            entry("z-folder", EntryKind::Dir, 0),
        ];
        assert_eq!(state.entries().len(), 3);

        state.set_show_hidden(false);
        assert_eq!(
            state
                .entries()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["z-folder", "a-visible"]
        );
        state.set_folders_first(false);
        assert_eq!(
            state
                .entries()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["a-visible", "z-folder"]
        );

        state.click("a-visible", Modifiers::default());
        state.set_layout(RowLayout::Icons);
        assert_eq!(state.selection().paths.len(), 1);
        assert!(state.selection().paths.contains("a-visible"));
        assert_eq!(state.layout(), RowLayout::Icons);
    }

    #[test]
    fn filtering_folds_case_accents_and_special_latin_letters() {
        assert!(matches("Résumé Søren", "resume soren"));
        assert!(matches("Straße", "strasse"));
    }

    #[test]
    fn selection_click_and_shift_replace_range_but_toggle_moves_anchor() {
        let order = ["a", "b", "c", "d"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let selected = click(&Selection::default(), &order, "b", Modifiers::default());
        let selected = click(
            &selected,
            &order,
            "d",
            Modifiers {
                shift: true,
                toggle: false,
            },
        );
        assert_eq!(
            selected.paths,
            ["b", "c", "d"].into_iter().map(str::to_owned).collect()
        );
        let selected = click(
            &selected,
            &order,
            "a",
            Modifiers {
                shift: false,
                toggle: true,
            },
        );
        assert_eq!(selected.anchor.as_deref(), Some("a"));
    }

    #[test]
    fn history_discards_forward_branch() {
        let history = History::initial("").go("a").go("b").back().go("c");
        assert_eq!(history.stack, ["", "a", "c"]);
        assert!(!history.can_forward());
    }

    #[test]
    fn location_parser_never_carries_a_password() {
        assert_eq!(
            parse_location("xsync://dorian@nas.local/media", None),
            Some(Location::Connect(ConnectionDraft {
                protocol: Protocol::Xsync,
                host: "nas.local".to_owned(),
                port: None,
                username: Some("dorian".to_owned()),
                share: "media".to_owned()
            }))
        );
        assert!(matches!(
            parse_location("ftp://host/x", None),
            Some(Location::Invalid { .. })
        ));
    }

    #[test]
    fn formatting_keeps_decimal_units_and_extension_rules() {
        assert_eq!(format_size(48_234_496_000), "48.2 GB");
        assert_eq!(format_size(16_000_000_000_000), "16 TB");
        assert_eq!(extension_of(".DS_Store"), None);
        assert_eq!(extension_of("Arrival.2016.MKV"), Some("mkv".to_owned()));
        assert_eq!(format_modified(Some(0)), "1 Jan 1970, 00:00");
        assert_eq!(format_modified_short(Some(0)), "1 Jan 00:00");
        assert_eq!(format_modified_grid(Some(0)), "01-01 00:00");
    }

    #[test]
    fn type_ahead_cycles_repeated_letters_and_refines_a_buffer() {
        let order = ["a", "blue", "black"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let names = ["Alpha", "Blue", "Black"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            type_ahead(&order, &names, "b", None, None),
            Some("blue".to_owned())
        );
        assert_eq!(
            type_ahead(&order, &names, "b", Some("blue"), None),
            Some("black".to_owned())
        );
        assert_eq!(
            type_ahead(&order, &names, "bl", Some("blue"), Some("a")),
            Some("blue".to_owned())
        );
    }

    #[test]
    fn permission_gate_uses_server_writability() {
        let context = PermissionContext {
            writable: false,
            reason: "Read-only export",
            has_selection: true,
            editable: None,
        };
        assert_eq!(
            gate(Affordance::Rename, context),
            Gate {
                disabled: true,
                reason: Some("Read-only export".to_owned())
            }
        );
        assert!(!gate(Affordance::Download, context).disabled);
        assert!(!accepts_drop(PermissionContext {
            has_selection: false,
            ..context
        }));
    }
}
