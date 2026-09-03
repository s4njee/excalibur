//! An in-memory backend, so the shell can be built and tested without a server.
//!
//! This exists for two jobs. The first is the M0 demo: the design has to be
//! reproducible from fixed data before any protocol works. The second outlives
//! it — every frontend test needs a backend that fails on demand, and a real
//! server is a poor way to produce a read-only mount or a dropped connection.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use super::{
    Attrs, Capacity, Caps, Entry, EntryKind, Health, Listing, MountInfo, MountStatus, ShareBackend,
    WriteMode,
};
use crate::error::{EngineError, Result};
use crate::kind::classify_entry;

/// A file or directory in the mock tree.
#[derive(Debug, Clone)]
struct Node {
    kind: EntryKind,
    size: u64,
    modified_ms: i64,
    contents: Vec<u8>,
}

/// A backend that answers from memory, with failures you can ask for.
pub struct MockBackend {
    /// Path to node. A `BTreeMap` so listings come out in a stable order and a
    /// test that compares two runs is comparing behaviour, not hash seeds.
    nodes: Mutex<BTreeMap<String, Node>>,
    mount: Mutex<MountInfo>,
    capacity: Capacity,
    latency: Option<Duration>,
    /// Calls remaining before every request starts failing. `u64::MAX` is off.
    calls_until_failure: AtomicU64,
}

impl MockBackend {
    /// An empty share, writable, with nothing in it.
    #[must_use]
    pub fn empty() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            String::new(),
            Node {
                kind: EntryKind::Dir,
                size: 0,
                modified_ms: 0,
                contents: Vec::new(),
            },
        );
        Self {
            nodes: Mutex::new(nodes),
            mount: Mutex::new(MountInfo {
                status: MountStatus::Ready,
                writable: true,
                reason: String::new(),
                protocol_label: "Mock".to_owned(),
                security_label: "none".to_owned(),
                options: String::new(),
                capacity: None,
                caps: Caps {
                    random_access: true,
                    resume_write: true,
                    atomic_rename: true,
                    symlinks: true,
                    watch: false,
                },
            }),
            capacity: Capacity {
                free: 2_400_000_000_000,
                total: 16_000_000_000_000,
            },
            latency: None,
            calls_until_failure: AtomicU64::new(u64::MAX),
        }
    }

    /// The share the design mocks: `smb://nas.local/media`, thirteen items.
    ///
    /// The numbers are the ones in the handoff, so a screenshot of this can be
    /// compared against the design directly rather than approximately.
    ///
    /// # Panics
    ///
    /// Panics if the freshly created backend's locks are already poisoned,
    /// which would mean a thread panicked inside this constructor.
    #[must_use]
    pub fn media_share() -> Self {
        let backend = Self::empty();
        {
            let mut nodes = backend.nodes.lock().expect("fresh mutex");
            // 2026-08-31T18:24Z, the instant the design's mock shows. It
            // renders in the viewer's timezone, so the displayed clock matches
            // the mock only in UTC; the point is a fixed, reproducible value.
            let base = 1_788_200_640_000_i64;
            let mut add = |path: &str, kind: EntryKind, size: u64, offset: i64| {
                nodes.insert(
                    path.to_owned(),
                    Node {
                        kind,
                        size,
                        modified_ms: base + offset,
                        contents: Vec::new(),
                    },
                );
            };
            add("Documentaries", EntryKind::Dir, 0, -86_400_000);
            add("Series", EntryKind::Dir, 0, -172_800_000);
            add("Transcodes", EntryKind::Dir, 0, -3_600_000);
            add("Arrival.2016.mkv", EntryKind::File, 48_234_496_000, 0);
            add("Arrival.2016.srt", EntryKind::File, 74_216, -1_000);
            add(
                "Blade.Runner.2049.mkv",
                EntryKind::File,
                61_680_312_320,
                -7_200_000,
            );
            add("Blade.Runner.2049.srt", EntryKind::File, 91_004, -7_201_000);
            add(
                "Dune.Part.Two.mkv",
                EntryKind::File,
                54_112_223_232,
                -14_400_000,
            );
            add(
                "Interstellar.mkv",
                EntryKind::File,
                44_982_075_392,
                -21_600_000,
            );
            add("Moon.2009.mp4", EntryKind::File, 8_412_774_400, -28_800_000);
            add(
                "Solaris.1972.mkv",
                EntryKind::File,
                39_126_437_888,
                -36_000_000,
            );
            add("cover-art.jpg", EntryKind::File, 2_284_112, -43_200_000);
            add(".DS_Store", EntryKind::File, 6_148, -43_300_000);
        }
        {
            let mut mount = backend.mount.lock().expect("fresh mutex");
            "SMB3".clone_into(&mut mount.protocol_label);
            "aes-128-gcm".clone_into(&mut mount.security_label);
            mount.capacity = Some(backend.capacity);
        }
        backend
    }

    /// Serve the share read-only, with `reason` shown in the status bar.
    ///
    /// # Panics
    ///
    /// Panics if the mount lock is poisoned, which needs a thread to have
    /// panicked while holding it.
    #[must_use]
    pub fn read_only(self, reason: &str) -> Self {
        {
            let mut mount = self.mount.lock().expect("fresh mutex");
            mount.writable = false;
            reason.clone_into(&mut mount.reason);
        }
        self
    }

    /// Label this mount as another protocol, for the dark read-only screen.
    ///
    /// # Panics
    ///
    /// As [`Self::read_only`].
    #[must_use]
    pub fn labelled(self, protocol: &str, security: &str, options: &str) -> Self {
        {
            let mut mount = self.mount.lock().expect("fresh mutex");
            protocol.clone_into(&mut mount.protocol_label);
            security.clone_into(&mut mount.security_label);
            options.clone_into(&mut mount.options);
        }
        self
    }

    /// Delay every call, to exercise the loading states.
    #[must_use]
    pub const fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = Some(latency);
        self
    }

    /// Fail every call after `calls` more have succeeded.
    #[must_use]
    pub fn failing_after(self, calls: u64) -> Self {
        self.calls_until_failure.store(calls, Ordering::Relaxed);
        self
    }

    /// Apply latency and the failure counter. Every method starts here.
    async fn gate(&self) -> Result<()> {
        if let Some(latency) = self.latency {
            tokio::time::sleep(latency).await;
        }
        let remaining = self.calls_until_failure.load(Ordering::Relaxed);
        if remaining == 0 {
            return Err(EngineError::Disconnected(
                "mock backend was asked to fail".to_owned(),
            ));
        }
        if remaining != u64::MAX {
            self.calls_until_failure
                .store(remaining - 1, Ordering::Relaxed);
        }
        Ok(())
    }

    fn refuse_if_read_only(&self, path: &str) -> Result<()> {
        let mount = self.mount.lock().map_err(|_| poisoned())?;
        if mount.writable {
            return Ok(());
        }
        // Refused here as well as in the UI. The UI gating is what a user sees;
        // this is what makes a bug in it harmless.
        Err(EngineError::ReadOnly(format!("{path}: {}", mount.reason)))
    }

    fn snapshot(&self) -> Result<MountInfo> {
        self.mount.lock().map_err(|_| poisoned()).map(|m| m.clone())
    }
}

fn poisoned() -> EngineError {
    EngineError::Io("mock backend state is poisoned".to_owned())
}

/// The parent directory of a path within the share, `""` for the root.
fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn base_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

#[async_trait]
impl ShareBackend for MockBackend {
    fn mount_info(&self) -> MountInfo {
        self.snapshot().unwrap_or_else(|_| MountInfo {
            status: MountStatus::Error,
            writable: false,
            reason: "mock backend state is poisoned".to_owned(),
            protocol_label: "Mock".to_owned(),
            security_label: "none".to_owned(),
            options: String::new(),
            capacity: None,
            caps: self.caps(),
        })
    }

    fn caps(&self) -> Caps {
        Caps {
            random_access: true,
            resume_write: true,
            atomic_rename: true,
            symlinks: true,
            watch: false,
        }
    }

    async fn capacity(&self) -> Result<Capacity> {
        self.gate().await?;
        Ok(self.capacity)
    }

    async fn list_dir(&self, path: &str) -> Result<Listing> {
        self.gate().await?;
        let nodes = self.nodes.lock().map_err(|_| poisoned())?;
        let directory = path.trim_matches('/');
        if !nodes.contains_key(directory) {
            return Err(EngineError::NotFound(path.to_owned()));
        }
        let entries = nodes
            .iter()
            .filter(|(candidate, _)| !candidate.is_empty() && parent_of(candidate) == directory)
            .map(|(candidate, node)| {
                let name = base_name(candidate).to_owned();
                let kind = classify_entry(&name, node.kind);
                Entry {
                    kind_label: kind.label.clone(),
                    kind_category: kind.category,
                    hidden: name.starts_with('.'),
                    attrs: Attrs {
                        // The mock has no protocol behind it to ask.
                        change_cookie: None,
                        kind: node.kind,
                        size: node.size,
                        modified_ms: Some(node.modified_ms),
                        mode: Some(if node.kind == EntryKind::Dir {
                            0o755
                        } else {
                            0o644
                        }),
                        link_target: None,
                    },
                    name,
                    path: candidate.clone(),
                }
            })
            .collect();
        Ok(Listing {
            entries,
            mount: self.snapshot()?,
        })
    }

    async fn stat(&self, path: &str) -> Result<Attrs> {
        self.gate().await?;
        let nodes = self.nodes.lock().map_err(|_| poisoned())?;
        nodes
            .get(path.trim_matches('/'))
            .map(|node| Attrs {
                change_cookie: None,
                kind: node.kind,
                size: node.size,
                modified_ms: Some(node.modified_ms),
                mode: Some(0o644),
                link_target: None,
            })
            .ok_or_else(|| EngineError::NotFound(path.to_owned()))
    }

    async fn read_range(&self, path: &str, offset: u64, length: u32) -> Result<Vec<u8>> {
        self.gate().await?;
        let nodes = self.nodes.lock().map_err(|_| poisoned())?;
        let node = nodes
            .get(path.trim_matches('/'))
            .ok_or_else(|| EngineError::NotFound(path.to_owned()))?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(node.contents.len());
        let end = start
            .saturating_add(length as usize)
            .min(node.contents.len());
        Ok(node.contents[start..end].to_vec())
    }

    async fn write_range(&self, path: &str, mode: WriteMode, data: &[u8]) -> Result<u64> {
        self.gate().await?;
        self.refuse_if_read_only(path)?;
        let key = path.trim_matches('/').to_owned();
        let mut nodes = self.nodes.lock().map_err(|_| poisoned())?;
        let node = nodes.entry(key).or_insert_with(|| Node {
            kind: EntryKind::File,
            size: 0,
            modified_ms: 0,
            contents: Vec::new(),
        });
        let offset = match mode {
            WriteMode::Create => {
                node.contents.clear();
                0
            }
            WriteMode::Resume { offset } => usize::try_from(offset).unwrap_or(usize::MAX),
        };
        if node.contents.len() < offset + data.len() {
            node.contents.resize(offset + data.len(), 0);
        }
        node.contents[offset..offset + data.len()].copy_from_slice(data);
        node.size = node.contents.len() as u64;
        Ok(data.len() as u64)
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        self.gate().await?;
        self.refuse_if_read_only(path)?;
        let key = path.trim_matches('/').to_owned();
        let mut nodes = self.nodes.lock().map_err(|_| poisoned())?;
        if nodes.contains_key(&key) {
            return Err(EngineError::AlreadyExists(path.to_owned()));
        }
        nodes.insert(
            key,
            Node {
                kind: EntryKind::Dir,
                size: 0,
                modified_ms: 0,
                contents: Vec::new(),
            },
        );
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<()> {
        self.gate().await?;
        self.refuse_if_read_only(from)?;
        let (from_key, to_key) = (
            from.trim_matches('/').to_owned(),
            to.trim_matches('/').to_owned(),
        );
        let mut nodes = self.nodes.lock().map_err(|_| poisoned())?;
        if !replace && nodes.contains_key(&to_key) {
            return Err(EngineError::AlreadyExists(to.to_owned()));
        }
        let node = nodes
            .remove(&from_key)
            .ok_or_else(|| EngineError::NotFound(from.to_owned()))?;
        nodes.insert(to_key, node);
        Ok(())
    }

    async fn remove(&self, path: &str, recursive: bool) -> Result<()> {
        self.gate().await?;
        self.refuse_if_read_only(path)?;
        let key = path.trim_matches('/').to_owned();
        let mut nodes = self.nodes.lock().map_err(|_| poisoned())?;
        if !nodes.contains_key(&key) {
            return Err(EngineError::NotFound(path.to_owned()));
        }
        let descendants: Vec<String> = nodes
            .keys()
            .filter(|candidate| candidate.starts_with(&format!("{key}/")))
            .cloned()
            .collect();
        if !descendants.is_empty() && !recursive {
            return Err(EngineError::Unsupported(format!(
                "{path} is not empty; pass recursive"
            )));
        }
        for descendant in descendants {
            nodes.remove(&descendant);
        }
        nodes.remove(&key);
        Ok(())
    }

    async fn ping(&self) -> Result<Health> {
        match self.gate().await {
            Ok(()) => Ok(Health::Healthy),
            Err(_) => Ok(Health::Failed),
        }
    }
}
