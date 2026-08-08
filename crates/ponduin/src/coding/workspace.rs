use crate::coding::outcome::ActionFailureKind;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Canonical security boundary for all internal coding-agent path operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodingWorkspace {
    root: PathBuf,
}

impl CodingWorkspace {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let requested = root.as_ref();
        if requested.as_os_str().is_empty() {
            return Err(WorkspaceError::EmptyRoot);
        }
        if !requested.is_absolute() {
            return Err(WorkspaceError::RootNotAbsolute(requested.to_path_buf()));
        }

        let root = requested
            .canonicalize()
            .map_err(|source| WorkspaceError::RootUnavailable {
                path: requested.to_path_buf(),
                source,
            })?;
        if !root.is_dir() {
            return Err(WorkspaceError::RootNotDirectory(root));
        }

        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a path that must already exist.
    pub fn resolve_existing(&self, path: impl AsRef<Path>) -> Result<PathBuf, WorkspaceError> {
        let requested = self.lexical_candidate(path.as_ref())?;
        let resolved =
            requested
                .canonicalize()
                .map_err(|source| WorkspaceError::PathUnavailable {
                    path: requested.clone(),
                    source,
                })?;
        self.require_inside(&resolved)?;
        Ok(resolved)
    }

    /// Resolve a path that may not exist yet.
    ///
    /// The nearest existing ancestor is canonicalized so a symlink cannot
    /// redirect a future write outside the workspace.
    pub fn resolve_for_write(&self, path: impl AsRef<Path>) -> Result<PathBuf, WorkspaceError> {
        let requested = self.lexical_candidate(path.as_ref())?;
        let (existing_ancestor, missing_suffix) = nearest_existing_ancestor(&requested)?;
        let canonical_ancestor =
            existing_ancestor
                .canonicalize()
                .map_err(|source| WorkspaceError::PathUnavailable {
                    path: existing_ancestor.clone(),
                    source,
                })?;
        self.require_inside(&canonical_ancestor)?;

        let resolved = missing_suffix
            .into_iter()
            .fold(canonical_ancestor, |path, component| path.join(component));
        self.require_inside(&resolved)?;
        Ok(resolved)
    }

    pub fn relative_path(&self, path: impl AsRef<Path>) -> Result<PathBuf, WorkspaceError> {
        let resolved = self.resolve_existing(path)?;
        resolved
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .map_err(|_| WorkspaceError::OutsideWorkspace(resolved))
    }

    fn lexical_candidate(&self, requested: &Path) -> Result<PathBuf, WorkspaceError> {
        if requested.as_os_str().is_empty() {
            return Err(WorkspaceError::EmptyPath);
        }
        if requested
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(WorkspaceError::ParentTraversal(requested.to_path_buf()));
        }

        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.root.join(requested)
        };
        Ok(candidate)
    }

    fn require_inside(&self, path: &Path) -> Result<(), WorkspaceError> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(WorkspaceError::OutsideWorkspace(path.to_path_buf()))
        }
    }
}

fn nearest_existing_ancestor(path: &Path) -> Result<(PathBuf, Vec<OsString>), WorkspaceError> {
    let mut ancestor = path.to_path_buf();
    let mut suffix = Vec::new();

    loop {
        match ancestor.symlink_metadata() {
            Ok(_) => break,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let component = ancestor
                    .file_name()
                    .ok_or_else(|| WorkspaceError::NoExistingAncestor(path.to_path_buf()))?;
                suffix.push(component.to_os_string());
                if !ancestor.pop() {
                    return Err(WorkspaceError::NoExistingAncestor(path.to_path_buf()));
                }
            }
            Err(source) => {
                return Err(WorkspaceError::PathUnavailable {
                    path: ancestor,
                    source,
                });
            }
        }
    }

    suffix.reverse();
    Ok((ancestor, suffix))
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("the coding workspace root is empty")]
    EmptyRoot,
    #[error("the coding workspace root must be absolute: {0}")]
    RootNotAbsolute(PathBuf),
    #[error("the coding workspace root is unavailable at {path}: {source}")]
    RootUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the coding workspace root is not a directory: {0}")]
    RootNotDirectory(PathBuf),
    #[error("the requested path is empty")]
    EmptyPath,
    #[error("parent traversal is not allowed in coding paths: {0}")]
    ParentTraversal(PathBuf),
    #[error("the requested path is outside the coding workspace: {0}")]
    OutsideWorkspace(PathBuf),
    #[error("the requested path is unavailable at {path}: {source}")]
    PathUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the requested path has no existing ancestor: {0}")]
    NoExistingAncestor(PathBuf),
}

impl WorkspaceError {
    pub(crate) fn failure_kind(&self) -> ActionFailureKind {
        match self {
            Self::RootUnavailable { source, .. } | Self::PathUnavailable { source, .. }
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                ActionFailureKind::ResourceMissing
            }
            Self::RootNotDirectory(_) | Self::NoExistingAncestor(_) => {
                ActionFailureKind::ResourceMissing
            }
            Self::OutsideWorkspace(_) | Self::ParentTraversal(_) => {
                ActionFailureKind::PolicyBlocked
            }
            Self::RootUnavailable { .. } | Self::PathUnavailable { .. } => {
                ActionFailureKind::TransientFailure
            }
            Self::EmptyRoot | Self::RootNotAbsolute(_) | Self::EmptyPath => {
                ActionFailureKind::InvalidArguments
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding::outcome::ActionFailureKind;
    use std::fs;

    #[test]
    fn requires_an_absolute_existing_directory() {
        assert!(matches!(
            CodingWorkspace::new("relative"),
            Err(WorkspaceError::RootNotAbsolute(_))
        ));

        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("file");
        fs::write(&file, "content").unwrap();
        assert!(matches!(
            CodingWorkspace::new(&file),
            Err(WorkspaceError::RootNotDirectory(_))
        ));
    }

    #[test]
    fn classifies_missing_and_policy_workspace_failures() {
        assert_eq!(
            WorkspaceError::NoExistingAncestor(PathBuf::from("missing/file")).failure_kind(),
            ActionFailureKind::ResourceMissing
        );
        assert_eq!(
            WorkspaceError::ParentTraversal(PathBuf::from("../outside")).failure_kind(),
            ActionFailureKind::PolicyBlocked
        );
        assert_eq!(
            WorkspaceError::EmptyPath.failure_kind(),
            ActionFailureKind::InvalidArguments
        );
    }

    #[test]
    fn resolves_existing_and_future_paths_inside_workspace() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        let nested = root.join("src");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("lib.rs"), "pub fn value() {}").unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();

        assert_eq!(
            workspace.resolve_existing("src/lib.rs").unwrap(),
            nested.join("lib.rs").canonicalize().unwrap()
        );
        assert_eq!(
            workspace.resolve_for_write("src/new/module.rs").unwrap(),
            nested.canonicalize().unwrap().join("new/module.rs")
        );
    }

    #[cfg(unix)]
    #[test]
    fn accepts_absolute_paths_through_an_in_workspace_root_alias() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        let alias = temp_dir.path().join("workspace-alias");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("existing.txt"), "content").unwrap();
        symlink(&root, &alias).unwrap();
        let workspace = CodingWorkspace::new(&alias).unwrap();

        assert_eq!(
            workspace
                .resolve_existing(alias.join("existing.txt"))
                .unwrap(),
            root.canonicalize().unwrap().join("existing.txt")
        );
        assert_eq!(
            workspace.resolve_for_write(alias.join("new.txt")).unwrap(),
            root.canonicalize().unwrap().join("new.txt")
        );
    }

    #[test]
    fn rejects_parent_traversal_and_absolute_external_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();

        assert!(matches!(
            workspace.resolve_for_write("../outside"),
            Err(WorkspaceError::ParentTraversal(_))
        ));
        assert!(matches!(
            workspace.resolve_for_write(temp_dir.path().join("outside")),
            Err(WorkspaceError::OutsideWorkspace(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_existing_and_future_paths_through_escaping_symlink() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        let outside = temp_dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret"), "secret").unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();

        assert!(matches!(
            workspace.resolve_existing("escape/secret"),
            Err(WorkspaceError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            workspace.resolve_for_write("escape/new/file"),
            Err(WorkspaceError::OutsideWorkspace(_))
        ));

        symlink(
            temp_dir.path().join("missing-outside"),
            root.join("dangling-escape"),
        )
        .unwrap();
        assert!(matches!(
            workspace.resolve_for_write("dangling-escape/new/file"),
            Err(WorkspaceError::PathUnavailable { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_existing_and_future_paths_through_escaping_symlink() {
        use std::os::windows::fs::symlink_dir;

        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        let outside = temp_dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret"), "secret").unwrap();
        if symlink_dir(&outside, root.join("escape")).is_err() {
            return;
        }
        let workspace = CodingWorkspace::new(&root).unwrap();

        assert!(matches!(
            workspace.resolve_existing("escape/secret"),
            Err(WorkspaceError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            workspace.resolve_for_write("escape/new/file"),
            Err(WorkspaceError::OutsideWorkspace(_))
        ));

        if symlink_dir(
            temp_dir.path().join("missing-outside"),
            root.join("dangling-escape"),
        )
        .is_ok()
        {
            assert!(matches!(
                workspace.resolve_for_write("dangling-escape/new/file"),
                Err(WorkspaceError::PathUnavailable { .. })
            ));
        }
    }

    #[test]
    fn returns_workspace_relative_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("workspace");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("README.md"), "read me").unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();

        assert_eq!(
            workspace.relative_path("README.md").unwrap(),
            PathBuf::from("README.md")
        );
    }
}
