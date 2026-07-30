use std::path::Path;

/// Conservative classification for files whose contents must not be included
/// in ordinary coding-agent search or context.
pub fn is_sensitive_path(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let normalized = file_name.to_ascii_lowercase();

    normalized == ".env"
        || normalized.starts_with(".env.")
        || matches!(
            normalized.as_str(),
            "id_rsa"
                | "id_dsa"
                | "id_ecdsa"
                | "id_ed25519"
                | "credentials"
                | "credentials.json"
                | "service-account.json"
                | "service_account.json"
                | "secrets.yaml"
                | "secrets.yml"
        )
        || matches!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("key" | "pem" | "p12" | "pfx" | "jks" | "keystore")
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_secret_files_without_blocking_source_names() {
        for path in [
            ".env",
            ".env.local",
            "server.pem",
            "private.key",
            ".ssh/id_ed25519",
            "config/credentials.json",
            "config/secrets.yaml",
        ] {
            assert!(is_sensitive_path(Path::new(path)), "{path}");
        }

        for path in [
            "src/secrets.rs",
            "src/credentials.py",
            "docs/environment.md",
            "public-key.txt",
        ] {
            assert!(!is_sensitive_path(Path::new(path)), "{path}");
        }
    }
}
