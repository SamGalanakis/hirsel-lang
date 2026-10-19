//! Byte-oriented source locations (LANG-1, FAIL-1); columns count Unicode scalars.

#[cfg(test)]
hirsel_guard::install!();

use std::path::{Path, PathBuf};

/// Session-local index into a source map, never a definition identity (PROG-1).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct FileId(pub u32);

/// Half-open UTF-8 byte range in one file (FAIL-1). Empty ranges mark insertions.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}
impl Span {
    pub fn new(file: FileId, start: u32, end: u32) -> Option<Self> {
        (start <= end).then_some(Self { file, start, end })
    }
}

/// One-based line and Unicode-scalar column, not terminal display cells (SEM-3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

/// Immutable source and its line-start index (LANG-1, SEM-3).
#[derive(Clone, Debug)]
pub struct SourceFile {
    /// Physical path for diagnostics and reading source; never emit into artifacts.
    pub path: PathBuf,
    logical_path: PathBuf,
    text: String,
    line_starts: Vec<u32>,
}
impl SourceFile {
    /// Synthetic DWARF root, identical on every host. GDB remaps the package
    /// prefix in relative file names to that package's local manifest directory.
    pub const COMPILATION_DIRECTORY: &str = "/hirsel";
    /// Root-independent source name for artifact strings and DWARF. Debuggers
    /// remap the package prefix to the local manifest directory (like
    /// `-fdebug-prefix-map`); the physical path remains available to diagnostics.
    pub fn logical_path(&self) -> &Path {
        &self.logical_path
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn location(&self, offset: u32) -> Option<LineColumn> {
        let offset_usize = offset as usize;
        if !self.text.is_char_boundary(offset_usize) {
            return None;
        }
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let start = self.line_starts[line] as usize;
        Some(LineColumn {
            line: line as u32 + 1,
            column: self.text[start..offset_usize].chars().count() as u32 + 1,
        })
    }
    pub fn line(&self, line: u32) -> Option<&str> {
        let index = line.checked_sub(1)? as usize;
        let start = *self.line_starts.get(index)? as usize;
        let end = self
            .line_starts
            .get(index + 1)
            .map_or(self.text.len(), |&v| v as usize);
        Some(self.text[start..end].trim_end_matches(['\r', '\n']))
    }
}

/// Vec-backed immutable file snapshots; IDs are valid only in this map (PROG-5).
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}
impl SourceMap {
    /// Returns None when the u32 representation cannot contain this file/map.
    pub fn add(&mut self, path: impl AsRef<Path>, text: String) -> Option<FileId> {
        let logical = Path::new("standalone@0").join(
            path.as_ref()
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("<source>")),
        );
        self.add_logical(path, logical, text)
    }
    /// Add a snapshot with a package-qualified, manifest-relative artifact name.
    pub fn add_logical(
        &mut self,
        path: impl AsRef<Path>,
        logical_path: impl AsRef<Path>,
        text: String,
    ) -> Option<FileId> {
        u32::try_from(text.len()).ok()?;
        let id = FileId(u32::try_from(self.files.len()).ok()?);
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        self.files.push(SourceFile {
            path: path.as_ref().to_owned(),
            logical_path: logical_path.as_ref().to_owned(),
            text,
            line_starts,
        });
        Some(id)
    }
    pub fn file(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }
    pub fn location(&self, span: Span) -> Option<LineColumn> {
        self.slice(span)?;
        self.file(span.file)?.location(span.start)
    }
    pub fn slice(&self, span: Span) -> Option<&str> {
        self.file(span.file)?
            .text
            .get(span.start as usize..span.end as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_crlf_eof_and_invalid_ranges() {
        let mut sources = SourceMap::default();
        let f = sources.add("a.hir", "é\r\nx\n".into()).unwrap();
        let source = sources.file(f).unwrap();
        assert_eq!(source.location(4), Some(LineColumn { line: 2, column: 1 }));
        assert_eq!(source.location(2).unwrap().column, 2);
        assert_eq!(source.location(6).unwrap().line, 3);
        assert_eq!(source.location(1), None);
        assert_eq!(source.location(7), None);
        assert_eq!(source.line(1), Some("é"));
        assert_eq!(
            sources.slice(Span {
                file: f,
                start: 2,
                end: 1
            }),
            None
        );
    }
}
