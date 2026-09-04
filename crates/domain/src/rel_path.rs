//! A validated path relative to the sync root, shared by both sides.
//!
//! Always forward slashes, never absolute, never `.`/`..` components, never
//! empty components, never NUL. Being a newtype means the planner and the
//! executor cannot be handed a path that could escape the sync folder.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelPathError {
    #[error("path is empty")]
    Empty,
    #[error("path is absolute")]
    Absolute,
    #[error("path component {0:?} is not allowed")]
    BadComponent(String),
}

/// A relative path such as `Documents/Notes/todo.md`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelPath(String);

impl RelPath {
    /// Parse and validate. Backslashes are not separators (they are legal in
    /// file names on Linux and on iCloud), so only `/` splits components.
    pub fn new(raw: &str) -> Result<Self, RelPathError> {
        if raw.is_empty() {
            return Err(RelPathError::Empty);
        }
        if raw.starts_with('/') {
            return Err(RelPathError::Absolute);
        }
        let trimmed = raw.strip_suffix('/').unwrap_or(raw);
        for component in trimmed.split('/') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.contains('\0')
            {
                return Err(RelPathError::BadComponent(component.to_string()));
            }
        }
        Ok(Self(trimmed.to_string()))
    }

    /// Build a child path from a parent (or the root when `parent` is `None`).
    pub fn child(parent: Option<&RelPath>, name: &str) -> Result<Self, RelPathError> {
        match parent {
            Some(p) => Self::new(&format!("{}/{}", p.0, name)),
            None => Self::new(name),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Final component.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Parent path, `None` for a top-level entry.
    pub fn parent(&self) -> Option<RelPath> {
        self.0
            .rsplit_once('/')
            .map(|(head, _)| RelPath(head.to_string()))
    }

    /// Number of components; used to order creates shallow-first and deletes
    /// deep-first.
    pub fn depth(&self) -> usize {
        self.0.split('/').count()
    }

    /// True when `self` is strictly inside `ancestor`.
    pub fn is_inside(&self, ancestor: &RelPath) -> bool {
        self.0.len() > ancestor.0.len()
            && self.0.starts_with(&ancestor.0)
            && self.0.as_bytes()[ancestor.0.len()] == b'/'
    }

    /// The same path with the file name replaced (used for conflict copies).
    pub fn with_name(&self, name: &str) -> Result<Self, RelPathError> {
        Self::child(self.parent().as_ref(), name)
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for RelPath {
    type Error = RelPathError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<RelPath> for String {
    fn from(p: RelPath) -> Self {
        p.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_paths_and_strips_trailing_slash() {
        assert_eq!(RelPath::new("a").unwrap().as_str(), "a");
        assert_eq!(RelPath::new("a/b.txt").unwrap().as_str(), "a/b.txt");
        assert_eq!(RelPath::new("a/b/").unwrap().as_str(), "a/b");
        assert_eq!(
            RelPath::new("back\\slash name").unwrap().as_str(),
            "back\\slash name"
        );
    }

    #[test]
    fn rejects_escapes_and_junk() {
        assert_eq!(RelPath::new(""), Err(RelPathError::Empty));
        assert_eq!(RelPath::new("/abs"), Err(RelPathError::Absolute));
        assert_eq!(
            RelPath::new("a/../b"),
            Err(RelPathError::BadComponent("..".into()))
        );
        assert_eq!(
            RelPath::new("./a"),
            Err(RelPathError::BadComponent(".".into()))
        );
        assert_eq!(
            RelPath::new("a//b"),
            Err(RelPathError::BadComponent(String::new()))
        );
        assert!(RelPath::new("nul\0byte").is_err());
    }

    #[test]
    fn navigation_helpers() {
        let p = RelPath::new("Docs/Notes/todo.md").unwrap();
        assert_eq!(p.name(), "todo.md");
        assert_eq!(p.parent().unwrap().as_str(), "Docs/Notes");
        assert_eq!(p.depth(), 3);
        assert!(p.is_inside(&RelPath::new("Docs").unwrap()));
        assert!(p.is_inside(&RelPath::new("Docs/Notes").unwrap()));
        assert!(!p.is_inside(&RelPath::new("Doc").unwrap()));
        assert!(!p.is_inside(&p));
        assert_eq!(RelPath::new("top").unwrap().parent(), None);
        assert_eq!(
            p.with_name("todo (conflict).md").unwrap().as_str(),
            "Docs/Notes/todo (conflict).md"
        );
        assert_eq!(
            RelPath::child(None, "x").unwrap().as_str(),
            "x",
            "root child has no separator"
        );
    }

    #[test]
    fn serde_roundtrip_validates() {
        let ok: RelPath = serde_json::from_str("\"a/b\"").unwrap();
        assert_eq!(ok.as_str(), "a/b");
        assert!(serde_json::from_str::<RelPath>("\"../x\"").is_err());
    }
}
