use crate::coding::repository::{ManifestKind, RepositoryProfile};
use crate::coding::workspace::CodingWorkspace;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_MANIFEST_BYTES: u64 = 1_024 * 1_024;
const MAX_DEPENDENCIES: usize = 500;
const MAX_WARNINGS: usize = 20;

/// Side-effect-free project, dependency, and validation command discovery.
#[derive(Debug)]
pub struct ProjectDiscovery;

impl ProjectDiscovery {
    pub fn discover(
        workspace: &CodingWorkspace,
        max_files: usize,
    ) -> Result<ProjectCapabilities, ProjectError> {
        let profile = RepositoryProfile::discover(workspace, max_files)?;
        let mut projects = BTreeMap::<(PathBuf, Ecosystem), ProjectAccumulator>::new();
        let mut warnings = profile.warnings.clone();

        for manifest in profile.manifests {
            let ecosystem = ecosystem_for_manifest(manifest.kind);
            let root = manifest
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let project = projects
                .entry((root.clone(), ecosystem))
                .or_insert_with(|| ProjectAccumulator::new(root, ecosystem));
            project.manifests.insert(manifest.path.clone());

            match read_manifest(workspace, &manifest.path) {
                Ok(contents) => project.inspect_manifest(workspace, manifest.kind, &contents),
                Err(error) => record_warning(
                    &mut warnings,
                    format!("{}: {error}", manifest.path.display()),
                ),
            }
        }

        let mut projects = projects
            .into_values()
            .map(ProjectAccumulator::finish)
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| {
            left.root
                .cmp(&right.root)
                .then(left.ecosystem.cmp(&right.ecosystem))
        });
        let ci_files = discover_ci_files(workspace);

        Ok(ProjectCapabilities {
            projects,
            ci_files,
            scanned_files: profile.scanned_files,
            truncated: profile.truncated,
            warnings,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCapabilities {
    pub projects: Vec<DetectedProject>,
    pub ci_files: Vec<PathBuf>,
    pub scanned_files: usize,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedProject {
    pub root: PathBuf,
    pub ecosystem: Ecosystem,
    pub manifests: Vec<PathBuf>,
    pub dependencies: Vec<String>,
    pub dependencies_truncated: bool,
    pub validation_commands: Vec<ValidationCommand>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Rust,
    Node,
    Python,
    Go,
    Maven,
    Gradle,
    Ruby,
    Swift,
    Php,
    Elixir,
    Dart,
    Dotnet,
    CMake,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCommand {
    pub id: String,
    pub kind: ValidationKind,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationKind {
    Build,
    Test,
    Lint,
    Format,
    Typecheck,
}

#[derive(Debug)]
struct ProjectAccumulator {
    root: PathBuf,
    ecosystem: Ecosystem,
    manifests: BTreeSet<PathBuf>,
    dependencies: BTreeSet<String>,
    commands: Vec<ValidationCommand>,
    warnings: Vec<String>,
}

impl ProjectAccumulator {
    fn new(root: PathBuf, ecosystem: Ecosystem) -> Self {
        Self {
            root,
            ecosystem,
            manifests: BTreeSet::new(),
            dependencies: BTreeSet::new(),
            commands: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn inspect_manifest(
        &mut self,
        workspace: &CodingWorkspace,
        kind: ManifestKind,
        contents: &str,
    ) {
        match kind {
            ManifestKind::Cargo => {
                self.dependencies.extend(parse_toml_dependencies(contents));
                self.add_standard_rust_commands();
            }
            ManifestKind::Node => self.inspect_package_json(workspace, contents),
            ManifestKind::PythonProject => self.inspect_pyproject(contents),
            ManifestKind::PythonRequirements => self.inspect_requirements(contents),
            ManifestKind::Go => {
                self.dependencies.extend(parse_go_dependencies(contents));
                self.add_command(ValidationKind::Test, "go", &["test", "./..."], "go.mod");
                self.add_command(ValidationKind::Lint, "go", &["vet", "./..."], "go.mod");
            }
            ManifestKind::Maven => {
                self.dependencies.extend(parse_maven_dependencies(contents));
                let program = self.wrapper_or("mvnw", "mvn", workspace);
                self.add_command(ValidationKind::Test, &program, &["test"], "pom.xml");
                self.add_command(ValidationKind::Build, &program, &["verify"], "pom.xml");
            }
            ManifestKind::Gradle => {
                self.dependencies
                    .extend(parse_gradle_dependencies(contents));
                let program = self.wrapper_or("gradlew", "gradle", workspace);
                self.add_command(ValidationKind::Test, &program, &["test"], "Gradle manifest");
                self.add_command(
                    ValidationKind::Build,
                    &program,
                    &["build"],
                    "Gradle manifest",
                );
            }
            ManifestKind::Ruby => {
                self.dependencies
                    .extend(parse_gemfile_dependencies(contents));
                self.add_command(
                    ValidationKind::Test,
                    "bundle",
                    &["exec", "rake", "test"],
                    "Gemfile",
                );
                self.add_command(
                    ValidationKind::Lint,
                    "bundle",
                    &["exec", "rubocop"],
                    "Gemfile",
                );
            }
            ManifestKind::Swift => {
                self.add_command(ValidationKind::Build, "swift", &["build"], "Package.swift");
                self.add_command(ValidationKind::Test, "swift", &["test"], "Package.swift");
            }
            ManifestKind::Php => self.inspect_composer_json(contents),
            ManifestKind::Elixir => {
                self.dependencies
                    .extend(parse_elixir_dependencies(contents));
                self.add_command(ValidationKind::Build, "mix", &["compile"], "mix.exs");
                self.add_command(ValidationKind::Test, "mix", &["test"], "mix.exs");
                self.add_command(
                    ValidationKind::Format,
                    "mix",
                    &["format", "--check-formatted"],
                    "mix.exs",
                );
            }
            ManifestKind::Dart => {
                self.dependencies.extend(parse_yaml_dependencies(contents));
                self.add_command(
                    ValidationKind::Typecheck,
                    "dart",
                    &["analyze"],
                    "pubspec.yaml",
                );
                self.add_command(ValidationKind::Test, "dart", &["test"], "pubspec.yaml");
                self.add_command(
                    ValidationKind::Format,
                    "dart",
                    &["format", "--output=none", "."],
                    "pubspec.yaml",
                );
            }
            ManifestKind::DotnetProject | ManifestKind::DotnetSolution => {
                self.dependencies
                    .extend(parse_dotnet_dependencies(contents));
                self.add_command(
                    ValidationKind::Build,
                    "dotnet",
                    &["build"],
                    "dotnet manifest",
                );
                self.add_command(ValidationKind::Test, "dotnet", &["test"], "dotnet manifest");
                self.add_command(
                    ValidationKind::Format,
                    "dotnet",
                    &["format", "--verify-no-changes"],
                    "dotnet manifest",
                );
            }
            ManifestKind::CMake => {
                self.add_command(
                    ValidationKind::Build,
                    "cmake",
                    &["-S", ".", "-B", "build"],
                    "CMakeLists.txt",
                );
                self.add_command(
                    ValidationKind::Test,
                    "ctest",
                    &["--test-dir", "build"],
                    "CMakeLists.txt",
                );
            }
        }
    }

    fn inspect_package_json(&mut self, workspace: &CodingWorkspace, contents: &str) {
        let package = match serde_json::from_str::<Value>(contents) {
            Ok(package) => package,
            Err(error) => {
                record_warning(&mut self.warnings, format!("invalid package.json: {error}"));
                return;
            }
        };
        for section in [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ] {
            if let Some(dependencies) = package.get(section).and_then(Value::as_object) {
                self.dependencies.extend(dependencies.keys().cloned());
            }
        }

        let manager = node_package_manager(workspace, &self.root);
        if let Some(scripts) = package.get("scripts").and_then(Value::as_object) {
            for name in scripts.keys() {
                let normalized = name.to_ascii_lowercase();
                let kind = if normalized == "test" || normalized.starts_with("test:") {
                    Some(ValidationKind::Test)
                } else if normalized == "lint" || normalized.starts_with("lint:") {
                    Some(ValidationKind::Lint)
                } else if normalized == "format"
                    || normalized.starts_with("format:")
                    || normalized == "fmt"
                {
                    Some(ValidationKind::Format)
                } else if matches!(
                    normalized.as_str(),
                    "typecheck" | "type-check" | "check:types"
                ) {
                    Some(ValidationKind::Typecheck)
                } else if normalized == "build" || normalized.starts_with("build:") {
                    Some(ValidationKind::Build)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    self.add_command(
                        kind,
                        manager,
                        &["run", name],
                        &format!("package.json script `{name}`"),
                    );
                }
            }
        }
    }

    fn inspect_pyproject(&mut self, contents: &str) {
        self.dependencies.extend(parse_toml_dependencies(contents));
        self.dependencies
            .extend(parse_pyproject_array_dependencies(contents));
        self.add_python_commands(contents);
    }

    fn inspect_requirements(&mut self, contents: &str) {
        self.dependencies
            .extend(contents.lines().filter_map(requirement_name));
        self.add_python_commands(contents);
    }

    fn add_python_commands(&mut self, evidence: &str) {
        let normalized = evidence.to_ascii_lowercase();
        if normalized.contains("pytest") {
            self.add_command(
                ValidationKind::Test,
                "python3",
                &["-m", "pytest"],
                "Python dependency/config",
            );
        }
        if normalized.contains("ruff") {
            self.add_command(
                ValidationKind::Lint,
                "python3",
                &["-m", "ruff", "check", "."],
                "Python dependency/config",
            );
            self.add_command(
                ValidationKind::Format,
                "python3",
                &["-m", "ruff", "format", "--check", "."],
                "Python dependency/config",
            );
        }
        if normalized.contains("mypy") {
            self.add_command(
                ValidationKind::Typecheck,
                "python3",
                &["-m", "mypy", "."],
                "Python dependency/config",
            );
        }
    }

    fn inspect_composer_json(&mut self, contents: &str) {
        let composer = match serde_json::from_str::<Value>(contents) {
            Ok(composer) => composer,
            Err(error) => {
                record_warning(
                    &mut self.warnings,
                    format!("invalid composer.json: {error}"),
                );
                return;
            }
        };
        for section in ["require", "require-dev"] {
            if let Some(dependencies) = composer.get(section).and_then(Value::as_object) {
                self.dependencies.extend(dependencies.keys().cloned());
            }
        }
        self.add_command(
            ValidationKind::Build,
            "composer",
            &["validate", "--no-interaction"],
            "composer.json",
        );
        if let Some(scripts) = composer.get("scripts").and_then(Value::as_object) {
            for name in scripts.keys() {
                if name == "test" || name.starts_with("test:") {
                    self.add_command(
                        ValidationKind::Test,
                        "composer",
                        &["run-script", name, "--no-interaction"],
                        &format!("composer.json script `{name}`"),
                    );
                }
            }
        }
    }

    fn add_standard_rust_commands(&mut self) {
        self.add_command(
            ValidationKind::Typecheck,
            "cargo",
            &["check", "--workspace"],
            "Cargo.toml",
        );
        self.add_command(
            ValidationKind::Test,
            "cargo",
            &["test", "--workspace"],
            "Cargo.toml",
        );
        self.add_command(
            ValidationKind::Lint,
            "cargo",
            &["clippy", "--workspace", "--all-targets"],
            "Cargo.toml",
        );
        self.add_command(
            ValidationKind::Format,
            "cargo",
            &["fmt", "--all", "--", "--check"],
            "Cargo.toml",
        );
    }

    fn wrapper_or(&self, wrapper: &str, fallback: &str, workspace: &CodingWorkspace) -> String {
        let relative = self.root.join(wrapper);
        if workspace.root().join(&relative).is_file() {
            format!("./{wrapper}")
        } else {
            fallback.to_string()
        }
    }

    fn add_command(&mut self, kind: ValidationKind, program: &str, args: &[&str], evidence: &str) {
        let cwd = if self.root.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            self.root.clone()
        };
        let args = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        let command = ValidationCommand {
            id: validation_command_id(kind, program, &args, &cwd),
            kind,
            program: program.to_string(),
            args,
            cwd,
            evidence: evidence.to_string(),
        };
        if !self.commands.contains(&command) {
            self.commands.push(command);
        }
    }

    fn finish(mut self) -> DetectedProject {
        self.commands.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then(left.program.cmp(&right.program))
                .then(left.args.cmp(&right.args))
        });
        let dependencies_truncated = self.dependencies.len() > MAX_DEPENDENCIES;
        DetectedProject {
            root: if self.root.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                self.root
            },
            ecosystem: self.ecosystem,
            manifests: self.manifests.into_iter().collect(),
            dependencies: self
                .dependencies
                .into_iter()
                .take(MAX_DEPENDENCIES)
                .collect(),
            dependencies_truncated,
            validation_commands: self.commands,
            warnings: self.warnings,
        }
    }
}

fn validation_command_id(
    kind: ValidationKind,
    program: &str,
    args: &[String],
    cwd: &Path,
) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(format!("{kind:?}").as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(program.as_bytes());
    bytes.push(0);
    for argument in args {
        bytes.extend_from_slice(argument.as_bytes());
        bytes.push(0);
    }
    bytes.extend_from_slice(cwd.to_string_lossy().as_bytes());
    format!("validation:{}", crate::coding::file::content_digest(&bytes))
}

fn ecosystem_for_manifest(kind: ManifestKind) -> Ecosystem {
    match kind {
        ManifestKind::Cargo => Ecosystem::Rust,
        ManifestKind::Node => Ecosystem::Node,
        ManifestKind::PythonProject | ManifestKind::PythonRequirements => Ecosystem::Python,
        ManifestKind::Go => Ecosystem::Go,
        ManifestKind::Maven => Ecosystem::Maven,
        ManifestKind::Gradle => Ecosystem::Gradle,
        ManifestKind::Ruby => Ecosystem::Ruby,
        ManifestKind::Swift => Ecosystem::Swift,
        ManifestKind::Php => Ecosystem::Php,
        ManifestKind::Elixir => Ecosystem::Elixir,
        ManifestKind::Dart => Ecosystem::Dart,
        ManifestKind::DotnetProject | ManifestKind::DotnetSolution => Ecosystem::Dotnet,
        ManifestKind::CMake => Ecosystem::CMake,
    }
}

fn read_manifest(workspace: &CodingWorkspace, path: &Path) -> Result<String, ProjectError> {
    let path = workspace.resolve_existing(path)?;
    let metadata = fs::metadata(&path)?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(ProjectError::ManifestTooLarge(metadata.len()));
    }
    Ok(fs::read_to_string(path)?)
}

fn parse_toml_dependencies(contents: &str) -> BTreeSet<String> {
    let mut dependencies = BTreeSet::new();
    let mut dependency_table = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            let table = line.trim_matches(['[', ']']).trim();
            dependency_table = table == "dependencies"
                || table == "dev-dependencies"
                || table == "build-dependencies"
                || table == "tool.poetry.dependencies"
                || table.ends_with(".dependencies")
                || table.ends_with(".dev-dependencies")
                || table.ends_with(".build-dependencies");
            continue;
        }
        if dependency_table {
            if let Some((name, _)) = line.split_once('=') {
                let name = name.trim().trim_matches(['"', '\'']);
                if !name.is_empty() && name != "python" {
                    dependencies.insert(name.to_string());
                }
            }
        }
    }
    dependencies
}

fn parse_pyproject_array_dependencies(contents: &str) -> BTreeSet<String> {
    let mut dependencies = BTreeSet::new();
    let mut collecting = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if !collecting && trimmed.starts_with("dependencies") && trimmed.contains('=') {
            collecting = true;
        }
        if collecting {
            for quoted in quoted_values(trimmed) {
                if let Some(name) = requirement_name(&quoted) {
                    dependencies.insert(name);
                }
            }
            if trimmed.contains(']') {
                collecting = false;
            }
        }
    }
    dependencies
}

fn parse_go_dependencies(contents: &str) -> BTreeSet<String> {
    let mut dependencies = BTreeSet::new();
    let mut block = false;
    for line in contents.lines() {
        let line = line.trim();
        if line == "require (" {
            block = true;
            continue;
        }
        if block && line == ")" {
            block = false;
            continue;
        }
        let dependency = if block {
            line.split_whitespace().next()
        } else {
            line.strip_prefix("require ")
                .and_then(|value| value.split_whitespace().next())
        };
        if let Some(dependency) = dependency.filter(|value| !value.starts_with("//")) {
            dependencies.insert(dependency.to_string());
        }
    }
    dependencies
}

fn parse_maven_dependencies(contents: &str) -> BTreeSet<String> {
    contents
        .split("<dependency>")
        .skip(1)
        .filter_map(|block| {
            let block = block.split("</dependency>").next()?;
            let group = xml_value(block, "groupId")?;
            let artifact = xml_value(block, "artifactId")?;
            Some(format!("{group}:{artifact}"))
        })
        .collect()
}

fn parse_gradle_dependencies(contents: &str) -> BTreeSet<String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let recognized = [
                "api",
                "implementation",
                "compileOnly",
                "runtimeOnly",
                "testImplementation",
                "kapt",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix));
            recognized.then(|| quoted_values(line).into_iter().next())?
        })
        .collect()
}

fn parse_gemfile_dependencies(contents: &str) -> BTreeSet<String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("gem ")
                .and_then(|rest| quoted_values(rest).into_iter().next())
        })
        .collect()
}

fn parse_elixir_dependencies(contents: &str) -> BTreeSet<String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("{:")?;
            let name = rest
                .split(|character: char| character == ',' || character.is_whitespace())
                .next()?;
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

fn parse_yaml_dependencies(contents: &str) -> BTreeSet<String> {
    let mut dependencies = BTreeSet::new();
    let mut in_dependencies = false;
    for line in contents.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if indent == 0 {
            in_dependencies = matches!(trimmed, "dependencies:" | "dev_dependencies:");
            continue;
        }
        if in_dependencies && indent > 0 {
            if let Some((name, _)) = trimmed.split_once(':') {
                if !name.is_empty() {
                    dependencies.insert(name.to_string());
                }
            }
        }
    }
    dependencies
}

fn parse_dotnet_dependencies(contents: &str) -> BTreeSet<String> {
    contents
        .split("<PackageReference")
        .skip(1)
        .filter_map(|block| xml_attribute(block, "Include"))
        .collect()
}

fn node_package_manager(workspace: &CodingWorkspace, root: &Path) -> &'static str {
    let root = workspace.root().join(root);
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("bun.lock").is_file() || root.join("bun.lockb").is_file() {
        "bun"
    } else {
        "npm"
    }
}

fn quoted_values(line: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut quote = None;
    let mut start = 0usize;
    for (index, character) in line.char_indices() {
        match quote {
            Some(expected) if character == expected => {
                if let Some(value) = line.get(start..index) {
                    values.push(value.to_string());
                }
                quote = None;
            }
            None if character == '"' || character == '\'' => {
                quote = Some(character);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    values
}

fn requirement_name(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty()
        || line.starts_with('#')
        || line.starts_with('-')
        || line.starts_with("http:")
        || line.starts_with("https:")
        || line.starts_with("git+")
    {
        return None;
    }
    let name = line
        .split(['<', '>', '=', '!', '~', ';', '[', ' '])
        .next()?
        .trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn xml_value<'a>(contents: &'a str, tag: &str) -> Option<&'a str> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = contents.find(&start_tag)? + start_tag.len();
    let remainder = contents.get(start..)?;
    let end = remainder.find(&end_tag)?;
    Some(remainder.get(..end)?.trim())
}

fn xml_attribute(contents: &str, attribute: &str) -> Option<String> {
    let marker = format!("{attribute}=");
    let start = contents.find(&marker)? + marker.len();
    let rest = contents.get(start..)?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = rest.get(quote.len_utf8()..)?;
    let end = value.find(quote)?;
    Some(value.get(..end)?.to_string())
}

fn discover_ci_files(workspace: &CodingWorkspace) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    for path in [
        ".gitlab-ci.yml",
        ".gitlab-ci.yaml",
        "azure-pipelines.yml",
        "azure-pipelines.yaml",
        "Jenkinsfile",
        ".circleci/config.yml",
    ] {
        if workspace
            .resolve_existing(path)
            .is_ok_and(|path| path.is_file())
        {
            files.insert(PathBuf::from(path));
        }
    }

    let workflows = workspace.root().join(".github/workflows");
    if let Ok(resolved) = workspace.resolve_existing(&workflows) {
        if let Ok(entries) = fs::read_dir(resolved) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && matches!(
                        path.extension().and_then(|extension| extension.to_str()),
                        Some("yml" | "yaml")
                    )
                {
                    if let Ok(relative) = path.strip_prefix(workspace.root()) {
                        files.insert(relative.to_path_buf());
                    }
                }
            }
        }
    }
    files.into_iter().collect()
}

fn record_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(warning);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error(transparent)]
    Repository(#[from] crate::coding::repository::RepositoryError),
    #[error(transparent)]
    Workspace(#[from] crate::coding::workspace::WorkspaceError),
    #[error("manifest is too large: {0} bytes")]
    ManifestTooLarge(u64),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_polyglot_dependencies_commands_and_ci_without_execution() {
        let temp_dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp_dir.path().join("web")).unwrap();
        fs::create_dir_all(temp_dir.path().join("api")).unwrap();
        fs::create_dir_all(temp_dir.path().join("service")).unwrap();
        fs::create_dir_all(temp_dir.path().join(".github/workflows")).unwrap();
        fs::write(
            temp_dir.path().join("Cargo.toml"),
            "[dependencies]\nserde = \"1\"\n[dev-dependencies]\ntempfile = \"3\"\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("web/package.json"),
            r#"{
                "scripts":{"test":"vitest","lint":"eslint .","build":"vite build"},
                "dependencies":{"react":"latest"},
                "devDependencies":{"vitest":"latest"}
            }"#,
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("web/pnpm-lock.yaml"),
            "lockfileVersion: 9",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("api/pyproject.toml"),
            "[project]\ndependencies = [\"fastapi>=1\", \"pytest\"]\n\
             [tool.ruff]\nline-length = 100\n[tool.mypy]\nstrict = true\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("service/go.mod"),
            "module example.test/service\nrequire (\n github.com/google/uuid v1.6.0\n)\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join(".github/workflows/ci.yml"),
            "on: [push]\n",
        )
        .unwrap();
        let workspace = CodingWorkspace::new(temp_dir.path()).unwrap();

        let capabilities = ProjectDiscovery::discover(&workspace, 1_000).unwrap();

        assert_eq!(capabilities.projects.len(), 4);
        assert_eq!(
            capabilities.ci_files,
            vec![PathBuf::from(".github/workflows/ci.yml")]
        );
        let rust = project(&capabilities, Ecosystem::Rust);
        assert!(rust.dependencies.contains(&"serde".to_string()));
        assert!(rust
            .validation_commands
            .iter()
            .any(|command| command.kind == ValidationKind::Lint && command.program == "cargo"));
        let node = project(&capabilities, Ecosystem::Node);
        assert!(node.dependencies.contains(&"react".to_string()));
        assert!(node
            .validation_commands
            .iter()
            .any(|command| command.kind == ValidationKind::Test
                && command.program == "pnpm"
                && command.args == ["run", "test"]));
        let python = project(&capabilities, Ecosystem::Python);
        assert!(python.dependencies.contains(&"fastapi".to_string()));
        assert!(python.validation_commands.iter().any(|command| {
            command.kind == ValidationKind::Format
                && command.args == ["-m", "ruff", "format", "--check", "."]
        }));
        let go = project(&capabilities, Ecosystem::Go);
        assert!(go
            .dependencies
            .contains(&"github.com/google/uuid".to_string()));
    }

    #[test]
    fn parses_enterprise_and_native_ecosystem_dependencies() {
        assert_eq!(
            parse_maven_dependencies(
                "<dependency><groupId>org.junit</groupId><artifactId>junit</artifactId></dependency>"
            ),
            BTreeSet::from(["org.junit:junit".to_string()])
        );
        assert_eq!(
            parse_gradle_dependencies("implementation(\"com.squareup.okio:okio:3.0\")"),
            BTreeSet::from(["com.squareup.okio:okio:3.0".to_string()])
        );
        assert_eq!(
            parse_dotnet_dependencies(r#"<PackageReference Include="Serilog" Version="4" />"#),
            BTreeSet::from(["Serilog".to_string()])
        );
    }

    fn project(capabilities: &ProjectCapabilities, ecosystem: Ecosystem) -> &DetectedProject {
        capabilities
            .projects
            .iter()
            .find(|project| project.ecosystem == ecosystem)
            .unwrap()
    }
}
