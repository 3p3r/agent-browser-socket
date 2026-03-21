use crate::bashkit_executor::SandboxFile;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::error::Error;
use std::path::{Component, Path, PathBuf};

const DEFAULT_SANDBOX_IGNORE_PATTERNS: &[&str] = &[
    ".git/",
    ".hg/",
    ".svn/",
    ".DS_Store",
    "Thumbs.db",
    "*.swp",
    "*.swo",
    "*~",
    "*.tmp",
    "*.temp",
    ".idea/",
    ".vscode/",
];

pub struct PreparedSandboxFile<'a> {
    pub file: &'a SandboxFile,
    pub relative_path: PathBuf,
}

struct SandboxSyncFilter {
    matcher: Gitignore,
}

impl SandboxSyncFilter {
    fn new(root: &Path, sandbox_ignore: Option<&Path>) -> Result<Self, Box<dyn Error>> {
        let mut builder = GitignoreBuilder::new(root);
        for pattern in DEFAULT_SANDBOX_IGNORE_PATTERNS {
            builder.add_line(None, pattern)?;
        }

        if let Some(ignore_path) = sandbox_ignore {
            if !ignore_path.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "sandbox ignore file does not exist or is not a file: {}",
                        ignore_path.display()
                    ),
                )
                .into());
            }

            if let Some(error) = builder.add(ignore_path) {
                return Err(Box::new(error));
            }
        }

        Ok(Self {
            matcher: builder.build()?,
        })
    }

    fn is_ignored(&self, relative_path: &Path) -> bool {
        self.matcher
            .matched_path_or_any_parents(relative_path, false)
            .is_ignore()
    }
}

pub fn sandbox_path_to_relative(path: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }

    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative)
    }
}

pub fn prepare_sandbox_files<'a>(
    root: &Path,
    sandbox_ignore: Option<&Path>,
    files: &'a [SandboxFile],
) -> Result<Vec<PreparedSandboxFile<'a>>, Box<dyn Error>> {
    let filter = SandboxSyncFilter::new(root, sandbox_ignore)?;
    let mut prepared = Vec::new();

    for file in files {
        let Some(relative_path) = sandbox_path_to_relative(file.path.as_path()) else {
            continue;
        };
        if filter.is_ignored(relative_path.as_path()) {
            continue;
        }

        prepared.push(PreparedSandboxFile {
            file,
            relative_path,
        });
    }

    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bashkit_executor::{SandboxFile, SandboxFileOrigin};

    fn fake_file(path: &str) -> SandboxFile {
        SandboxFile {
            path: PathBuf::from(path),
            data: b"x".to_vec(),
            mime_type: "text/plain".to_string(),
            origin: SandboxFileOrigin::Sandbox,
        }
    }

    #[test]
    fn preserves_relative_path_without_root_prefix() {
        let files = vec![fake_file("/tmp/report.txt")];
        let prepared = prepare_sandbox_files(Path::new("."), None, &files).unwrap();

        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].relative_path, PathBuf::from("tmp/report.txt"));
    }

    #[test]
    fn ignores_default_noise_files() {
        let files = vec![fake_file("/.DS_Store"), fake_file("/keep.txt")];
        let prepared = prepare_sandbox_files(Path::new("."), None, &files).unwrap();

        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].relative_path, PathBuf::from("keep.txt"));
    }

    #[test]
    fn applies_custom_ignore_file() {
        let root = std::env::temp_dir().join(format!(
            "abs-sandbox-files-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let ignore_path = root.join("sandbox.ignore");
        std::fs::write(&ignore_path, "*.log\n").unwrap();

        let files = vec![fake_file("/trace.log"), fake_file("/keep.txt")];
        let prepared =
            prepare_sandbox_files(root.as_path(), Some(ignore_path.as_path()), &files).unwrap();

        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].relative_path, PathBuf::from("keep.txt"));

        std::fs::remove_dir_all(root).ok();
    }
}
