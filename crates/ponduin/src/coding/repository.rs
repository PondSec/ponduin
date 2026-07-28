use crate::coding::workspace::CodingWorkspace;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_RECORDED_WALK_ERRORS: usize = 20;

/// Side-effect-free repository inventory used before deeper indexing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryProfile {
    pub root: PathBuf,
    pub version_control: VersionControl,
    pub manifests: Vec<ProjectManifest>,
    pub languages: BTreeMap<Language, usize>,
    pub scanned_files: usize,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

impl RepositoryProfile {
    pub fn discover(
        workspace: &CodingWorkspace,
        max_files: usize,
    ) -> Result<Self, RepositoryError> {
        if max_files == 0 {
            return Err(RepositoryError::InvalidFileLimit);
        }

        let root = workspace.root();
        let mut builder = WalkBuilder::new(root);
        builder
            .git_ignore(true)
            .git_exclude(true)
            .git_global(false)
            .parents(false)
            .require_git(false)
            .ignore(true)
            .hidden(true)
            .follow_links(false);

        let mut manifests = Vec::new();
        let mut languages = BTreeMap::new();
        let mut scanned_files = 0;
        let mut truncated = false;
        let mut warnings = Vec::new();

        for entry in builder.build() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    if warnings.len() < MAX_RECORDED_WALK_ERRORS {
                        warnings.push(error.to_string());
                    }
                    continue;
                }
            };
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            if scanned_files == max_files {
                truncated = true;
                break;
            }

            scanned_files += 1;
            let relative_path = match entry.path().strip_prefix(root) {
                Ok(path) => path.to_path_buf(),
                Err(_) => {
                    if warnings.len() < MAX_RECORDED_WALK_ERRORS {
                        warnings.push(format!(
                            "ignored path outside repository root: {}",
                            entry.path().display()
                        ));
                    }
                    continue;
                }
            };

            if let Some(kind) = manifest_kind(&relative_path) {
                manifests.push(ProjectManifest {
                    path: relative_path.clone(),
                    kind,
                });
            }
            if let Some(language) = language_for_path(&relative_path) {
                *languages.entry(language).or_insert(0) += 1;
            }
        }

        manifests.sort_by(|left, right| left.path.cmp(&right.path));
        warnings.sort();

        Ok(Self {
            root: root.to_path_buf(),
            version_control: detect_version_control(root),
            manifests,
            languages,
            scanned_files,
            truncated,
            warnings,
        })
    }

    pub fn is_mixed_language(&self) -> bool {
        self.languages.len() > 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionControl {
    Git,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub path: PathBuf,
    pub kind: ManifestKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestKind {
    Cargo,
    Node,
    PythonProject,
    PythonRequirements,
    Go,
    Maven,
    Gradle,
    Ruby,
    Swift,
    Php,
    Elixir,
    Dart,
    DotnetProject,
    DotnetSolution,
    CMake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    C,
    Cpp,
    CSharp,
    Css,
    Dart,
    Elixir,
    Erlang,
    FSharp,
    Go,
    Html,
    Java,
    JavaScript,
    Json,
    Kotlin,
    Lua,
    Markdown,
    Php,
    Python,
    Ruby,
    Rust,
    Scala,
    Shell,
    Sql,
    Svelte,
    Swift,
    Toml,
    TypeScript,
    Vue,
    Yaml,
}

fn detect_version_control(root: &Path) -> VersionControl {
    match root.join(".git").symlink_metadata() {
        Ok(metadata) if metadata.is_dir() || metadata.is_file() => VersionControl::Git,
        _ => VersionControl::None,
    }
}

fn manifest_kind(path: &Path) -> Option<ManifestKind> {
    let file_name = path.file_name()?.to_str()?;
    match file_name {
        "Cargo.toml" => Some(ManifestKind::Cargo),
        "package.json" => Some(ManifestKind::Node),
        "pyproject.toml" => Some(ManifestKind::PythonProject),
        "requirements.txt" => Some(ManifestKind::PythonRequirements),
        "go.mod" => Some(ManifestKind::Go),
        "pom.xml" => Some(ManifestKind::Maven),
        "build.gradle" | "build.gradle.kts" => Some(ManifestKind::Gradle),
        "Gemfile" => Some(ManifestKind::Ruby),
        "Package.swift" => Some(ManifestKind::Swift),
        "composer.json" => Some(ManifestKind::Php),
        "mix.exs" => Some(ManifestKind::Elixir),
        "pubspec.yaml" => Some(ManifestKind::Dart),
        "CMakeLists.txt" => Some(ManifestKind::CMake),
        _ if file_name.ends_with(".csproj") => Some(ManifestKind::DotnetProject),
        _ if file_name.ends_with(".sln") => Some(ManifestKind::DotnetSolution),
        _ => None,
    }
}

fn language_for_path(path: &Path) -> Option<Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "c" => Some(Language::C),
        "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx" => Some(Language::Cpp),
        "cs" => Some(Language::CSharp),
        "css" | "less" | "sass" | "scss" => Some(Language::Css),
        "dart" => Some(Language::Dart),
        "ex" | "exs" => Some(Language::Elixir),
        "erl" | "hrl" => Some(Language::Erlang),
        "fs" | "fsi" | "fsx" => Some(Language::FSharp),
        "go" => Some(Language::Go),
        "htm" | "html" => Some(Language::Html),
        "java" => Some(Language::Java),
        "cjs" | "js" | "jsx" | "mjs" => Some(Language::JavaScript),
        "json" | "jsonc" => Some(Language::Json),
        "kt" | "kts" => Some(Language::Kotlin),
        "lua" => Some(Language::Lua),
        "md" | "mdx" => Some(Language::Markdown),
        "php" => Some(Language::Php),
        "py" | "pyi" => Some(Language::Python),
        "rb" => Some(Language::Ruby),
        "rs" => Some(Language::Rust),
        "scala" | "sc" => Some(Language::Scala),
        "bash" | "fish" | "sh" | "zsh" => Some(Language::Shell),
        "sql" => Some(Language::Sql),
        "svelte" => Some(Language::Svelte),
        "swift" => Some(Language::Swift),
        "toml" => Some(Language::Toml),
        "ts" | "tsx" | "mts" | "cts" => Some(Language::TypeScript),
        "vue" => Some(Language::Vue),
        "yaml" | "yml" => Some(Language::Yaml),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("repository discovery requires a non-zero file limit")]
    InvalidFileLimit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_mixed_nested_projects_without_executing_manifests() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("repository");
        fs::create_dir_all(root.join("web/src")).unwrap();
        fs::create_dir_all(root.join("backend")).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"fixture\"").unwrap();
        fs::write(root.join("src.rs"), "fn main() {}").unwrap();
        fs::write(root.join("web/package.json"), "{\"scripts\":{}}").unwrap();
        fs::write(
            root.join("web/src/app.tsx"),
            "export const App = () => null",
        )
        .unwrap();
        fs::write(
            root.join("backend/pyproject.toml"),
            "[project]\nname = \"api\"",
        )
        .unwrap();
        fs::write(root.join("backend/app.py"), "def app(): pass").unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();

        let profile = RepositoryProfile::discover(&workspace, 100).unwrap();

        assert_eq!(profile.version_control, VersionControl::Git);
        assert_eq!(profile.manifests.len(), 3);
        assert_eq!(profile.languages.get(&Language::Rust), Some(&1));
        assert_eq!(profile.languages.get(&Language::TypeScript), Some(&1));
        assert_eq!(profile.languages.get(&Language::Python), Some(&1));
        assert!(profile.is_mixed_language());
        assert!(!profile.truncated);
    }

    #[test]
    fn recognizes_git_worktree_marker_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::write(temp_dir.path().join(".git"), "gitdir: /tmp/example").unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();

        let profile = RepositoryProfile::discover(&workspace, 10).unwrap();

        assert_eq!(profile.version_control, VersionControl::Git);
    }

    #[test]
    fn respects_repository_ignore_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir(temp_dir.path().join(".git")).unwrap();
        fs::create_dir_all(temp_dir.path().join(".git/objects")).unwrap();
        fs::create_dir(temp_dir.path().join("ignored")).unwrap();
        fs::write(temp_dir.path().join(".gitignore"), "ignored/\n").unwrap();
        fs::write(
            temp_dir.path().join(".git/objects/not_source.py"),
            "def internal(): pass",
        )
        .unwrap();
        fs::write(temp_dir.path().join("visible.rs"), "fn visible() {}").unwrap();
        fs::write(
            temp_dir.path().join("ignored/hidden.py"),
            "def hidden(): pass",
        )
        .unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();

        let profile = RepositoryProfile::discover(&workspace, 100).unwrap();

        assert_eq!(profile.languages.get(&Language::Rust), Some(&1));
        assert!(!profile.languages.contains_key(&Language::Python));
    }

    #[test]
    fn stops_at_the_file_limit_and_reports_truncation() {
        let temp_dir = tempfile::tempdir().unwrap();
        for index in 0..5 {
            fs::write(temp_dir.path().join(format!("{index}.rs")), "fn value() {}").unwrap();
        }
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();

        let profile = RepositoryProfile::discover(&workspace, 2).unwrap();

        assert_eq!(profile.scanned_files, 2);
        assert!(profile.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks_outside_the_workspace() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().join("repository");
        let outside = temp_dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("external.py"), "def external(): pass").unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        fs::write(root.join("local.rs"), "fn local() {}").unwrap();
        let workspace = CodingWorkspace::new(&root).unwrap();

        let profile = RepositoryProfile::discover(&workspace, 100).unwrap();

        assert_eq!(profile.languages.get(&Language::Rust), Some(&1));
        assert!(!profile.languages.contains_key(&Language::Python));
    }
}
