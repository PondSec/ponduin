use ponduin::coding::context::{ContextLimits, ContextPlanner};
use ponduin::coding::file::{FileReadOptions, FileSnapshot};
use ponduin::coding::intelligence::{IntelligenceLimits, RepositoryIntelligence};
use ponduin::coding::patch::{FileChange, MutationBatch, PatchError, PatchLimits, TextReplacement};
use ponduin::coding::process::ProcessLimits;
use ponduin::coding::project::{Ecosystem, ProjectCapabilities, ProjectDiscovery, ValidationKind};
use ponduin::coding::repository::{Language, RepositoryProfile};
use ponduin::coding::search::{RepositorySearch, SearchLimits, TextSearchRequest};
use ponduin::coding::validation::{ValidationService, ValidationStatus};
use ponduin::coding::{CodingWorkspace, PatchEngine};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const ORIGINAL_ERROR: &str = "user-not-found";
const UPDATED_ERROR: &str = "user-required";

fn write_fixture(root: &Path, path: &str, content: &str) {
    let destination = root.join(path);
    fs::create_dir_all(destination.parent().expect("fixture parent")).unwrap();
    fs::write(destination, content).unwrap();
}

fn create_polyglot_repository() -> tempfile::TempDir {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();

    write_fixture(
        root,
        "Cargo.toml",
        "[package]\nname = \"polyglot-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_fixture(
        root,
        "src/lib.rs",
        r#"pub fn normalize_user(name: &str) -> Result<String, &'static str> {
    if name.trim().is_empty() {
        Err("user-not-found")
    } else {
        Ok(name.trim().to_string())
    }
}

pub fn load_user(name: &str) -> Result<String, &'static str> {
    normalize_user(name)
}
"#,
    );
    write_fixture(
        root,
        "web/package.json",
        r#"{
  "scripts": {
    "typecheck": "tsc --noEmit",
    "test": "vitest run"
  },
  "dependencies": {
    "zod": "4"
  },
  "devDependencies": {
    "typescript": "5",
    "vitest": "4"
  }
}
"#,
    );
    write_fixture(
        root,
        "web/src/user.ts",
        r#"export function normalizeUser(name: string): string {
  if (name.trim().length === 0) {
    throw new Error("user-not-found");
  }
  return name.trim();
}

export function loadUser(name: string): string {
  return normalizeUser(name);
}
"#,
    );
    write_fixture(
        root,
        "api/pyproject.toml",
        "[project]\nname = \"polyglot-api\"\nversion = \"0.1.0\"\ndependencies = [\"fastapi>=0.100\"]\n\n[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n\n[tool.ruff]\nline-length = 100\n",
    );
    write_fixture(
        root,
        "api/user_service.py",
        r#"def normalize_user(name: str) -> str:
    if not name.strip():
        raise ValueError("user-not-found")
    return name.strip()


def load_user(name: str) -> str:
    return normalize_user(name)
"#,
    );
    write_fixture(
        root,
        "service/go.mod",
        "module example.test/polyglot/service\n\ngo 1.24\n",
    );
    write_fixture(
        root,
        "service/user.go",
        r#"package service

import "errors"

func NormalizeUser(name string) (string, error) {
	if name == "" {
		return "", errors.New("user-not-found")
	}
	return name, nil
}

func LoadUser(name string) (string, error) {
	return NormalizeUser(name)
}
"#,
    );
    write_fixture(
        root,
        "java/pom.xml",
        r#"<project>
  <modelVersion>4.0.0</modelVersion>
  <groupId>example.test</groupId>
  <artifactId>polyglot-java</artifactId>
  <version>0.1.0</version>
  <dependencies>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter</artifactId>
      <version>5.12.0</version>
    </dependency>
  </dependencies>
</project>
"#,
    );
    write_fixture(
        root,
        "java/src/main/java/UserService.java",
        r#"package example.test;

public final class UserService {
    public static String normalizeUser(String name) {
        if (name.isBlank()) {
            throw new IllegalArgumentException("user-not-found");
        }
        return name.trim();
    }

    public static String loadUser(String name) {
        return normalizeUser(name);
    }
}
"#,
    );

    for path in [
        "node_modules/untrusted/ignored.ts",
        "target/generated/ignored.rs",
        "dist/ignored.py",
        ".venv/ignored.py",
        ".cache/ignored.go",
    ] {
        write_fixture(root, path, "user-not-found\n");
    }
    write_fixture(root, ".env", "API_TOKEN=user-not-found\n");
    repository
}

fn project(capabilities: &ProjectCapabilities, ecosystem: Ecosystem) -> &str {
    capabilities
        .projects
        .iter()
        .find(|project| project.ecosystem == ecosystem)
        .map(|project| project.root.to_str().expect("UTF-8 fixture path"))
        .expect("detected project")
}

fn snapshots(workspace: &CodingWorkspace, paths: &[&str]) -> Vec<FileSnapshot> {
    paths
        .iter()
        .map(|path| {
            FileSnapshot::read(workspace, path, FileReadOptions::default())
                .expect("fixture snapshot")
        })
        .collect()
}

fn replace_error_batch(snapshots: &[FileSnapshot], replacement: &str) -> MutationBatch {
    MutationBatch {
        changes: snapshots
            .iter()
            .map(|snapshot| FileChange::Replace {
                path: snapshot.path.clone(),
                expected_digest: snapshot.digest.clone(),
                replacements: vec![TextReplacement {
                    old: ORIGINAL_ERROR.to_string(),
                    new: replacement.to_string(),
                    replace_all: false,
                }],
            })
            .collect(),
    }
}

#[tokio::test]
async fn analyzes_edits_validates_and_repairs_a_polyglot_repository() {
    let repository = create_polyglot_repository();
    let workspace = CodingWorkspace::new(repository.path()).unwrap();

    let profile = RepositoryProfile::discover(&workspace, 1_000).unwrap();
    for language in [
        Language::Rust,
        Language::TypeScript,
        Language::Python,
        Language::Go,
        Language::Java,
    ] {
        assert_eq!(profile.languages.get(&language), Some(&1));
    }
    assert!(profile.is_mixed_language());

    let capabilities = ProjectDiscovery::discover(&workspace, 1_000).unwrap();
    assert_eq!(capabilities.projects.len(), 5);
    assert_eq!(project(&capabilities, Ecosystem::Rust), ".");
    assert_eq!(project(&capabilities, Ecosystem::Node), "web");
    assert_eq!(project(&capabilities, Ecosystem::Python), "api");
    assert_eq!(project(&capabilities, Ecosystem::Go), "service");
    assert_eq!(project(&capabilities, Ecosystem::Maven), "java");

    let index = RepositoryIntelligence::build(&workspace, IntelligenceLimits::default()).unwrap();
    assert_eq!(index.files.len(), 5);
    for path in [
        "src/lib.rs",
        "web/src/user.ts",
        "api/user_service.py",
        "service/user.go",
        "java/src/main/java/UserService.java",
    ] {
        assert!(index.files.iter().any(|file| file.path == Path::new(path)));
    }
    assert_eq!(
        index
            .search_symbols("normalize_user", true, 10)
            .unwrap()
            .matches
            .len(),
        2
    );
    assert_eq!(
        index
            .search_symbols("normalizeUser", true, 10)
            .unwrap()
            .matches
            .len(),
        2
    );
    assert_eq!(
        index
            .references("normalize_user", 10)
            .unwrap()
            .matches
            .len(),
        2
    );
    assert_eq!(
        index.references("normalizeUser", 10).unwrap().matches.len(),
        2
    );
    assert_eq!(
        index.references("NormalizeUser", 10).unwrap().matches.len(),
        1
    );

    let search = RepositorySearch::new(&workspace);
    let matches = search
        .search_text(
            &TextSearchRequest {
                pattern: ORIGINAL_ERROR.to_string(),
                scope: PathBuf::from("."),
                regex: false,
                case_sensitive: true,
                include: Vec::new(),
            },
            SearchLimits::default(),
        )
        .unwrap();
    assert_eq!(matches.matches.len(), 5);
    assert!(matches
        .matches
        .iter()
        .all(|matched| !matched.path.starts_with("node_modules")));
    let sensitive = search
        .search_text(
            &TextSearchRequest {
                pattern: ORIGINAL_ERROR.to_string(),
                scope: PathBuf::from(".env"),
                regex: false,
                case_sensitive: true,
                include: Vec::new(),
            },
            SearchLimits::default(),
        )
        .unwrap();
    assert!(sensitive.matches.is_empty());
    assert_eq!(sensitive.skipped_sensitive, 1);

    let context = ContextPlanner::new(&workspace, &index)
        .prepare(
            "normalize user",
            ContextLimits {
                token_budget: 1_024,
                max_files: 5,
                max_file_bytes: 64 * 1_024,
                chunk_lines: 30,
                overlap_lines: 5,
            },
        )
        .unwrap();
    assert!(context.selected_files >= 3);
    assert!(context.used_tokens <= context.token_budget);
    assert!(!context
        .chunks
        .iter()
        .any(|chunk| chunk.path == Path::new(".env")));

    let paths = ["src/lib.rs", "web/src/user.ts", "api/user_service.py"];
    let original = snapshots(&workspace, &paths);
    let engine = PatchEngine::new(&workspace, PatchLimits::default());
    let prepared = engine
        .prepare(replace_error_batch(&original, UPDATED_ERROR))
        .unwrap();
    fs::write(
        workspace.root().join("api/user_service.py"),
        format!("{}# concurrent user edit\n", original[2].content),
    )
    .unwrap();

    let conflict = engine.apply(prepared).unwrap_err();
    assert!(matches!(conflict, PatchError::DigestConflict { .. }));
    assert_eq!(
        fs::read_to_string(workspace.root().join("src/lib.rs")).unwrap(),
        original[0].content
    );
    assert_eq!(
        fs::read_to_string(workspace.root().join("web/src/user.ts")).unwrap(),
        original[1].content
    );

    fs::write(
        workspace.root().join("api/user_service.py"),
        &original[2].content,
    )
    .unwrap();
    let current = snapshots(&workspace, &paths);
    let applied = engine
        .apply(
            engine
                .prepare(replace_error_batch(&current, UPDATED_ERROR))
                .unwrap(),
        )
        .unwrap();
    assert_eq!(applied.result.preview.files.len(), 3);
    for path in paths {
        assert!(fs::read_to_string(workspace.root().join(path))
            .unwrap()
            .contains(UPDATED_ERROR));
    }
    engine.rollback(applied.rollback).unwrap();
    for (path, snapshot) in paths.iter().zip(original.iter()) {
        assert_eq!(
            fs::read_to_string(workspace.root().join(path)).unwrap(),
            snapshot.content
        );
    }

    let typecheck_id = capabilities
        .projects
        .iter()
        .find(|project| project.ecosystem == Ecosystem::Rust)
        .and_then(|project| {
            project
                .validation_commands
                .iter()
                .find(|command| command.kind == ValidationKind::Typecheck)
        })
        .map(|command| command.id.clone())
        .expect("Rust typecheck command");
    let limits = ProcessLimits {
        timeout: Duration::from_secs(30),
        output_limit: 64 * 1_024,
    };
    let initial = ValidationService::run(&workspace, &capabilities, &typecheck_id, limits).await;
    assert_eq!(initial.status, ValidationStatus::Passed);

    let rust_snapshot =
        FileSnapshot::read(&workspace, "src/lib.rs", FileReadOptions::default()).unwrap();
    let broken = engine
        .apply(
            engine
                .prepare(MutationBatch {
                    changes: vec![FileChange::Replace {
                        path: PathBuf::from("src/lib.rs"),
                        expected_digest: rust_snapshot.digest,
                        replacements: vec![TextReplacement {
                            old: "Ok(name.trim().to_string())".to_string(),
                            new: "Ok(name.trim().to_string()".to_string(),
                            replace_all: false,
                        }],
                    }],
                })
                .unwrap(),
        )
        .unwrap();
    let failed = ValidationService::run(&workspace, &capabilities, &typecheck_id, limits).await;
    assert_eq!(failed.status, ValidationStatus::Failed);
    assert!(failed
        .output
        .as_ref()
        .is_some_and(|output| !output.diagnostics.diagnostics.is_empty()));

    engine.rollback(broken.rollback).unwrap();
    let repaired = ValidationService::run(&workspace, &capabilities, &typecheck_id, limits).await;
    assert_eq!(repaired.status, ValidationStatus::Passed);
}

#[test]
fn bounds_analysis_search_and_context_for_a_medium_repository() {
    let repository = tempfile::tempdir().unwrap();
    for index in 0..900 {
        let (directory, extension, source) = match index % 3 {
            0 => (
                "rust",
                "rs",
                format!("pub fn medium_handler_{index}() -> usize {{ {index} }}\n"),
            ),
            1 => (
                "python",
                "py",
                format!("def medium_handler_{index}() -> int:\n    return {index}\n"),
            ),
            _ => (
                "web",
                "ts",
                format!("export function mediumHandler{index}(): number {{ return {index}; }}\n"),
            ),
        };
        write_fixture(
            repository.path(),
            &format!("{directory}/module_{index}.{extension}"),
            &source,
        );
    }
    for index in 0..300 {
        write_fixture(
            repository.path(),
            &format!("node_modules/dependency_{index}/index.ts"),
            "export const MEDIUM_MATCH = 'excluded';\n",
        );
    }
    let workspace = CodingWorkspace::new(repository.path()).unwrap();

    let profile = RepositoryProfile::discover(&workspace, 200).unwrap();
    assert_eq!(profile.scanned_files, 200);
    assert!(profile.truncated);
    assert_eq!(profile.languages.values().sum::<usize>(), 200);

    let index = RepositoryIntelligence::build(
        &workspace,
        IntelligenceLimits {
            max_files: 300,
            max_file_bytes: 8 * 1_024,
            max_symbols: 1_000,
        },
    )
    .unwrap();
    assert_eq!(index.scanned_files, 300);
    assert_eq!(index.files.len(), 300);
    assert!(index.truncated);
    assert!(index
        .files
        .iter()
        .all(|file| !file.path.starts_with("node_modules")));

    let search = RepositorySearch::new(&workspace);
    let result = search
        .search_text(
            &TextSearchRequest {
                pattern: "medium".to_string(),
                scope: PathBuf::from("."),
                regex: false,
                case_sensitive: false,
                include: Vec::new(),
            },
            SearchLimits {
                max_results: 25,
                max_files: 100,
                max_file_bytes: 8 * 1_024,
                max_line_bytes: 1_024,
            },
        )
        .unwrap();
    assert_eq!(result.matches.len(), 25);
    assert!(result.truncated);
    assert!(result.scanned_files <= 100);
    assert!(result
        .matches
        .iter()
        .all(|matched| !matched.path.starts_with("node_modules")));

    let context = ContextPlanner::new(&workspace, &index)
        .prepare(
            "medium handler",
            ContextLimits {
                token_budget: 2_048,
                max_files: 8,
                max_file_bytes: 8 * 1_024,
                chunk_lines: 20,
                overlap_lines: 4,
            },
        )
        .unwrap();
    assert!(!context.chunks.is_empty());
    assert!(context.selected_files <= 8);
    assert!(context.used_tokens <= context.token_budget);
}
