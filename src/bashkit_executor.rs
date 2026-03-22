use crate::command_args::PreparedCommand;
use bashkit::{async_trait, Bash, Builtin, BuiltinContext, ExecResult, FileSystem, InMemoryFs};
use ftmi::extract_paths_from_text;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio as ProcessStdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Clone)]
pub struct BashkitExecutor {
    binary_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SandboxFile {
    pub path: PathBuf,
    pub data: Vec<u8>,
    pub mime_type: String,
    pub origin: SandboxFileOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFileOrigin {
    Sandbox,
    RealFs,
}

#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub files: Vec<SandboxFile>,
}

#[derive(Clone)]
struct AgentBrowserBuiltin {
    binary_path: PathBuf,
}

#[async_trait]
impl Builtin for AgentBrowserBuiltin {
    async fn execute(&self, ctx: BuiltinContext<'_>) -> bashkit::Result<ExecResult> {
        let mut command = Command::new(&self.binary_path);
        command
            .args(ctx.args)
            .stdout(ProcessStdio::piped())
            .stderr(ProcessStdio::piped());

        if ctx.stdin.is_some() {
            command.stdin(ProcessStdio::piped());
        } else {
            command.stdin(ProcessStdio::null());
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Ok(ExecResult::err(
                    format!("agent-browser: failed to spawn process: {error}\n"),
                    127,
                ))
            }
        };

        if let Some(stdin) = ctx.stdin {
            if let Some(mut child_stdin) = child.stdin.take() {
                if let Err(error) = child_stdin.write_all(stdin.as_bytes()).await {
                    return Ok(ExecResult::err(
                        format!("agent-browser: failed to write stdin: {error}\n"),
                        1,
                    ));
                }
            }
        }

        match child.wait_with_output().await {
            Ok(output) => Ok(ExecResult {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code().unwrap_or(1),
                ..Default::default()
            }),
            Err(error) => Ok(ExecResult::err(
                format!("agent-browser: failed to wait for process: {error}\n"),
                1,
            )),
        }
    }

    fn llm_hint(&self) -> Option<&'static str> {
        Some("agent-browser: execute agent-browser CLI commands")
    }
}

impl BashkitExecutor {
    pub fn new(binary_path: PathBuf) -> Self {
        Self { binary_path }
    }

    pub async fn execute(
        &self,
        command: &PreparedCommand,
        env: Option<&HashMap<String, String>>,
    ) -> ExecutionResult {
        let sandbox_fs = Arc::new(InMemoryFs::new());
        let baseline_files = collect_file_paths(sandbox_fs.as_ref()).await;

        let pre_existing: HashSet<PathBuf> = command
            .referenced_paths
            .iter()
            .cloned()
            .filter(|p| p.exists())
            .collect();

        let mut builder = Bash::builder().builtin(
            "agent-browser",
            Box::new(AgentBrowserBuiltin {
                binary_path: self.binary_path.clone(),
            }),
        );
        builder = builder.fs(sandbox_fs.clone());

        if let Some(env_vars) = env {
            for (key, value) in env_vars {
                builder = builder.env(key.clone(), value.clone());
            }
        }

        let mut bash = builder.build();
        match bash.exec(&command.script).await {
            Ok(result) => {
                let mut files = collect_new_files(sandbox_fs.as_ref(), &baseline_files).await;

                let output_paths: HashSet<PathBuf> =
                    extract_paths_from_text(&format!("{}\n{}", result.stdout, result.stderr))
                        .into_iter()
                        .map(PathBuf::from)
                        .collect();
                let all_paths: HashSet<PathBuf> = command
                    .referenced_paths
                    .iter()
                    .cloned()
                    .chain(output_paths)
                    .collect();
                files.extend(collect_real_files(&all_paths, &pre_existing));

                ExecutionResult {
                    stdout: result.stdout,
                    stderr: result.stderr,
                    exit_code: result.exit_code,
                    files,
                }
            }
            Err(error) => ExecutionResult {
                stdout: String::new(),
                stderr: format!("failed to execute bashkit script: {error}"),
                exit_code: -1,
                files: Vec::new(),
            },
        }
    }
}

async fn collect_file_paths(fs: &InMemoryFs) -> HashSet<PathBuf> {
    let mut files = HashSet::new();
    let mut pending_dirs = vec![PathBuf::from("/")];

    while let Some(dir) = pending_dirs.pop() {
        let entries = match fs.read_dir(dir.as_path()).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries {
            let path = child_path(dir.as_path(), &entry.name);
            let kind = entry.metadata.file_type;
            if kind.is_dir() {
                pending_dirs.push(path);
            } else if kind.is_file() || kind.is_fifo() {
                files.insert(path);
            }
        }
    }

    files
}

async fn collect_new_files(fs: &InMemoryFs, baseline_files: &HashSet<PathBuf>) -> Vec<SandboxFile> {
    let all_files = collect_file_paths(fs).await;
    let mut created: Vec<_> = all_files
        .into_iter()
        .filter(|path| !baseline_files.contains(path))
        .collect();
    created.sort();

    let mut files = Vec::new();
    for path in created {
        if let Ok(data) = fs.read_file(path.as_path()).await {
            files.push(SandboxFile {
                mime_type: guess_mime_type(path.as_path()),
                path,
                data,
                origin: SandboxFileOrigin::Sandbox,
            });
        }
    }

    files
}

fn child_path(parent: &Path, child_name: &str) -> PathBuf {
    if parent == Path::new("/") {
        PathBuf::from(format!("/{child_name}"))
    } else {
        parent.join(child_name)
    }
}

fn collect_real_files(
    paths: &HashSet<PathBuf>,
    pre_existing: &HashSet<PathBuf>,
) -> Vec<SandboxFile> {
    let mut files = Vec::new();
    let mut sorted: Vec<_> = paths.iter().collect();
    sorted.sort();

    for path in sorted {
        if pre_existing.contains(path.as_path()) {
            continue;
        }
        if path.is_file() {
            if let Ok(data) = std::fs::read(path) {
                files.push(SandboxFile {
                    mime_type: guess_mime_type(path),
                    path: path.clone(),
                    data,
                    origin: SandboxFileOrigin::RealFs,
                });
            }
        }
    }

    files
}

fn guess_mime_type(path: &Path) -> String {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return "application/octet-stream".to_string();
    };

    match ext.to_ascii_lowercase().as_str() {
        "txt" | "md" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "js" => "text/javascript",
        "css" => "text/css",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "yaml" | "yml" => "application/yaml",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_args::prepare_command;

    #[tokio::test]
    async fn execute_collects_new_root_file() {
        let executor = BashkitExecutor::new(PathBuf::from("agent-browser"));
        let command = prepare_command("echo hello > /output.txt").unwrap();
        let result = executor.execute(&command, None).await;

        assert_eq!(result.exit_code, 0);
        let output = result
            .files
            .iter()
            .find(|file| file.path == PathBuf::from("/output.txt"));
        assert!(output.is_some());
        let output = output.unwrap();
        assert_eq!(output.data, b"hello\n");
        assert_eq!(output.mime_type, "text/plain");
        assert_eq!(output.origin, SandboxFileOrigin::Sandbox);
    }

    #[tokio::test]
    async fn execute_collects_file_created_under_default_directory() {
        let executor = BashkitExecutor::new(PathBuf::from("agent-browser"));
        let command = prepare_command("echo id,name > /tmp/report.csv").unwrap();
        let result = executor.execute(&command, None).await;

        assert_eq!(result.exit_code, 0);
        assert!(result
            .files
            .iter()
            .any(|file| file.path == PathBuf::from("/tmp/report.csv")));
    }

    #[tokio::test]
    async fn execute_supports_basic_shell_variables_and_pipelines() {
        let executor = BashkitExecutor::new(PathBuf::from("agent-browser"));
        let command = prepare_command(
            "name=world && echo hello-$name | cat && echo saved-$name > /shell.txt",
        )
        .unwrap();
        let result = executor.execute(&command, None).await;

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello-world\n");

        let output = result
            .files
            .iter()
            .find(|file| file.path == PathBuf::from("/shell.txt"))
            .expect("shell output file");
        assert_eq!(output.data, b"saved-world\n");
        assert_eq!(output.origin, SandboxFileOrigin::Sandbox);
    }

    #[test]
    fn collect_real_files_skips_pre_existing() {
        let tmp = std::env::temp_dir().join(format!(
            "oatmeal-real-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let existing = tmp.join("old.txt");
        std::fs::write(&existing, b"old").unwrap();
        let created = tmp.join("new.txt");
        std::fs::write(&created, b"new").unwrap();

        let paths: HashSet<PathBuf> = [existing.clone(), created.clone()].into_iter().collect();

        let pre_existing: HashSet<PathBuf> = [existing.clone()].into_iter().collect();
        let files = collect_real_files(&paths, &pre_existing);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].data, b"new");
        assert_eq!(files[0].origin, SandboxFileOrigin::RealFs);

        std::fs::remove_dir_all(&tmp).ok();
    }
}
