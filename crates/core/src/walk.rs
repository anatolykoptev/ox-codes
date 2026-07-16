use std::path::{Path, PathBuf};

use globset::GlobSet;
use ignore::WalkBuilder;

/// Default cap on the number of files a single walk visits. Callers that
/// pass `None` for `max_files` walk without a count cap (but may still be
/// bounded by `max_file_bytes`).
pub const DEFAULT_MAX_FILES: usize = 2000;

/// Default per-file byte cap. Callers that pass `None` for `max_file_bytes`
/// walk files of any size. This is ox-core's own constant — it is independent
/// of any peer engine's defaults (ox-dataflow keeps its own).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// Budget for a filtered directory walk. Both fields are optional: `None`
/// means "no cap on that dimension". Callers pass concrete values; ox-core
/// hardcodes no request-specific literal.
#[derive(Debug, Clone, Copy)]
pub struct WalkBudget {
    /// Maximum number of qualifying files to yield. `None` ⇒ walk all.
    pub max_files: Option<usize>,
    /// Maximum per-file size in bytes. `None` ⇒ no per-file byte cap.
    /// Plumbed through to `ignore::WalkBuilder::max_filesize`.
    pub max_file_bytes: Option<u64>,
}

/// Iterator over the files in a directory tree, filtered by extension and
/// glob include/exclude sets, with optional count and per-file byte caps.
///
/// Yields `(PathBuf, relative_path_string, Metadata)` triples. The walk is
/// sorted by file path (deterministic) and applies `ignore`'s standard
/// filters (gitignore, hidden, etc.).
pub struct FilteredWalk<'a> {
    walk: ignore::Walk,
    root: &'a Path,
    exts: Option<&'a [&'a str]>,
    include: Option<&'a GlobSet>,
    exclude: Option<&'a GlobSet>,
    max_files: Option<usize>,
    count: usize,
    truncated: bool,
}

impl<'a> FilteredWalk<'a> {
    /// `true` iff a qualifying file existed beyond position `max_files` cap.
    /// Checked AFTER pulling a qualifying item, not before — so a walk that
    /// naturally exhausts at exactly the cap reports `false` (not truncated).
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

impl Iterator for FilteredWalk<'_> {
    type Item = (PathBuf, String, std::fs::Metadata);

    fn next(&mut self) -> Option<Self::Item> {
        for result in self.walk.by_ref() {
            let entry = result.ok()?;
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }

            let path = entry.path().to_path_buf();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if let Some(exts) = self.exts
                && !exts.contains(&ext)
            {
                continue;
            }

            let rel_path = path.strip_prefix(self.root).unwrap_or(&path);
            if let Some(inc) = self.include
                && !inc.is_match(rel_path)
            {
                continue;
            }
            if let Some(exc) = self.exclude
                && exc.is_match(rel_path)
            {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Cap check AFTER pulling a qualifying item, not before. This
            // distinguishes "walk naturally exhausted at exactly cap" (not
            // truncated) from "a qualifying file existed beyond position cap"
            // (truncated). Checking at the top would set truncated=true even
            // when nothing was skipped (e.g. a repo with exactly max_files
            // qualifying files).
            if self.max_files.is_some_and(|cap| self.count >= cap) {
                self.truncated = true;
                return None;
            }

            self.count += 1;
            let rel_str = rel_path.to_string_lossy().into_owned();
            return Some((path, rel_str, metadata));
        }

        None
    }
}

/// Build a filtered, sorted walk over `root`.
///
/// - `exts: None` ⇒ no extension filter (walk all file types).
/// - `exts: Some(&[..])` ⇒ only yield files whose extension is in the slice.
/// - `include`/`exclude` are optional `GlobSet` filters applied to the path
///   relative to `root`.
/// - `budget.max_file_bytes` is plumbed through to
///   `ignore::WalkBuilder::max_filesize` (the byte cap is enforced by
///   `ignore`, not hand-rolled). `None` ⇒ no byte cap.
/// - `budget.max_files` is the count cap (enforced in `FilteredWalk::next`).
pub fn filtered_walk<'a>(
    root: &'a Path,
    exts: Option<&'a [&'a str]>,
    include: Option<&'a GlobSet>,
    exclude: Option<&'a GlobSet>,
    budget: WalkBudget,
) -> FilteredWalk<'a> {
    FilteredWalk {
        walk: WalkBuilder::new(root)
            .standard_filters(true)
            .max_filesize(budget.max_file_bytes)
            .sort_by_file_path(|a, b| a.cmp(b))
            .build(),
        root,
        exts,
        include,
        exclude,
        max_files: budget.max_files,
        count: 0,
        truncated: false,
    }
}
