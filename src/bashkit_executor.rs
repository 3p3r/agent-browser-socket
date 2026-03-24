use crate::command_args::PreparedCommand;
use bashkit::{async_trait, Bash, Builtin, BuiltinContext, ExecResult, FileSystem, InMemoryFs};
use ftmi::extract_paths_from_text;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio as ProcessStdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

static CAPTURE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct BashkitExecutor {
    binary_path: PathBuf,
    path_policy: AgentBrowserPathPolicy,
}

#[derive(Debug, Clone)]
pub enum AgentBrowserPathPolicy {
    McpCacheRooted(PathBuf),
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
    path_policy: AgentBrowserPathPolicy,
    output_path_hints: Arc<Mutex<Vec<PathBuf>>>,
}

#[async_trait]
impl Builtin for AgentBrowserBuiltin {
    async fn execute(&self, ctx: BuiltinContext<'_>) -> bashkit::Result<ExecResult> {
        let command_args = match rewrite_agent_browser_args(
            ctx.args,
            &self.path_policy,
            &self.output_path_hints,
        ) {
            Ok(args) => args,
            Err(result) => return Ok(result),
        };
        let stdout_path = capture_file_path("stdout");
        let stderr_path = capture_file_path("stderr");
        let stdout_file = match File::create(&stdout_path) {
            Ok(file) => file,
            Err(error) => {
                return Ok(ExecResult::err(
                    format!("agent-browser: failed to create stdout capture: {error}\n"),
                    1,
                ))
            }
        };
        let stderr_file = match File::create(&stderr_path) {
            Ok(file) => file,
            Err(error) => {
                let _ = std::fs::remove_file(&stdout_path);
                return Ok(ExecResult::err(
                    format!("agent-browser: failed to create stderr capture: {error}\n"),
                    1,
                ));
            }
        };
        let mut command = Command::new(&self.binary_path);
        command
            .args(&command_args)
            .stdout(ProcessStdio::from(stdout_file))
            .stderr(ProcessStdio::from(stderr_file))
            .env("AGENT_BROWSER_SESSION", "safe");
        if let Some(binary_dir) = self.binary_path.parent() {
            command.env("AGENT_BROWSER_HOME", binary_dir);
        }

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

        let wait_result = child.wait().await;
        let stdout = read_capture_file(&stdout_path);
        let stderr = read_capture_file(&stderr_path);
        let _ = std::fs::remove_file(&stdout_path);
        let _ = std::fs::remove_file(&stderr_path);

        match wait_result {
            Ok(status) => Ok(ExecResult {
                stdout,
                stderr,
                exit_code: status.code().unwrap_or(1),
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

fn capture_file_path(kind: &str) -> PathBuf {
    let id = CAPTURE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "oatmeal-agent-browser-{kind}-{}-{id}.log",
        std::process::id()
    ))
}

fn read_capture_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(_) => String::new(),
    }
}

impl BashkitExecutor {
    #[cfg(test)]
    pub fn new(binary_path: PathBuf) -> Self {
        Self::new_with_path_policy(
            binary_path,
            AgentBrowserPathPolicy::McpCacheRooted(std::env::temp_dir()),
        )
    }

    pub fn new_with_path_policy(binary_path: PathBuf, path_policy: AgentBrowserPathPolicy) -> Self {
        Self {
            binary_path,
            path_policy,
        }
    }

    pub async fn execute(
        &self,
        command: &PreparedCommand,
        env: Option<&HashMap<String, String>>,
    ) -> ExecutionResult {
        let sandbox_fs = Arc::new(InMemoryFs::new());
        let baseline_files = collect_file_paths(sandbox_fs.as_ref()).await;
        let output_path_hints = Arc::new(Mutex::new(Vec::new()));

        let pre_existing: HashSet<PathBuf> = command
            .referenced_paths
            .iter()
            .filter(|p| p.exists())
            .cloned()
            .collect();

        let mut builder = Bash::builder().python().builtin(
            "agent-browser",
            Box::new(AgentBrowserBuiltin {
                binary_path: self.binary_path.clone(),
                path_policy: self.path_policy.clone(),
                output_path_hints: output_path_hints.clone(),
            }),
        );
        builder = builder.builtin(
            "ab",
            Box::new(AgentBrowserBuiltin {
                binary_path: self.binary_path.clone(),
                path_policy: self.path_policy.clone(),
                output_path_hints: output_path_hints.clone(),
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
                    .chain(
                        output_path_hints
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .clone(),
                    )
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

fn rewrite_agent_browser_args(
    args: &[String],
    path_policy: &AgentBrowserPathPolicy,
    output_path_hints: &Arc<Mutex<Vec<PathBuf>>>,
) -> Result<Vec<String>, ExecResult> {
    let Some(output_index) = output_arg_index(args) else {
        return Ok(args.to_vec());
    };

    let Some(rewritten_path) = rewrite_output_path(&args[output_index], path_policy) else {
        return Ok(args.to_vec());
    };

    if let Some(parent) = rewritten_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return Err(ExecResult::err(
                format!(
                    "agent-browser: failed to create output directory {}: {error}\n",
                    parent.display()
                ),
                1,
            ));
        }
    }

    output_path_hints
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(rewritten_path.clone());

    let mut rewritten = args.to_vec();
    rewritten[output_index] = rewritten_path.display().to_string();
    Ok(rewritten)
}

fn output_arg_index(args: &[String]) -> Option<usize> {
    let subcommand_index = args.iter().position(|arg| !arg.starts_with('-'))?;
    let subcommand = args.get(subcommand_index)?;
    if subcommand != "screenshot" && subcommand != "pdf" {
        return None;
    }

    let positional_indices: Vec<_> = args
        .iter()
        .enumerate()
        .skip(subcommand_index + 1)
        .filter_map(|(index, arg)| (!arg.starts_with('-')).then_some(index))
        .collect();

    if positional_indices.len() >= 2 {
        return positional_indices.last().copied();
    }

    let single_index = positional_indices.first().copied()?;
    looks_like_output_path(&args[single_index]).then_some(single_index)
}

fn looks_like_output_path(arg: &str) -> bool {
    if arg.contains("://") || arg.starts_with("about:") {
        return false;
    }

    arg.starts_with('/')
        || arg.starts_with("./")
        || arg.starts_with("../")
        || arg.starts_with('~')
        || arg.contains('\\')
        || arg.contains('/')
        || is_windows_drive_path(arg)
        || [
            ".png", ".jpg", ".jpeg", ".webp", ".pdf", ".csv", ".json", ".yaml", ".yml",
        ]
        .iter()
        .any(|suffix| arg.to_ascii_lowercase().ends_with(suffix))
}

fn rewrite_output_path(arg: &str, path_policy: &AgentBrowserPathPolicy) -> Option<PathBuf> {
    let AgentBrowserPathPolicy::McpCacheRooted(cache_root) = path_policy;
    Some(cache_root.join(path_to_cache_relative(arg)))
}

fn path_to_cache_relative(raw: &str) -> PathBuf {
    if let Some(relative) = windows_drive_relative(raw) {
        return relative;
    }

    if raw.starts_with('/') || raw.starts_with('\\') {
        return split_path_components(raw);
    }

    if raw.contains('\\') {
        return split_path_components(raw);
    }

    PathBuf::from(raw)
}

fn windows_drive_relative(raw: &str) -> Option<PathBuf> {
    if !is_windows_drive_path(raw) {
        return None;
    }

    let mut relative = PathBuf::new();
    relative.push(&raw[..1]);
    for segment in raw[2..].split(['/', '\\']) {
        if !segment.is_empty() {
            relative.push(segment);
        }
    }
    Some(relative)
}

fn is_windows_drive_path(raw: &str) -> bool {
    raw.len() >= 2 && raw.as_bytes()[1] == b':' && raw.as_bytes()[0].is_ascii_alphabetic()
}

fn split_path_components(raw: &str) -> PathBuf {
    let mut relative = PathBuf::new();
    for segment in raw.split(['/', '\\']) {
        if !segment.is_empty() {
            relative.push(segment);
        }
    }
    relative
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
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

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

    #[tokio::test]
    async fn execute_supports_inline_python() {
        let executor = BashkitExecutor::new(PathBuf::from("agent-browser"));
        let command = prepare_command("python3 -c \"print(2 ** 10)\"").unwrap();
        let result = executor.execute(&command, None).await;

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "1024\n");
    }

    #[tokio::test]
    async fn execute_supports_python_vfs_round_trip() {
        let executor = BashkitExecutor::new(PathBuf::from("agent-browser"));
        let command = prepare_command(
            "echo important > /shared.txt && python3 -c \"from pathlib import Path; content = Path('/shared.txt').read_text().strip(); _ = Path('/result.txt').write_text(f'value={content}\\n')\"",
        )
        .unwrap();
        let result = executor.execute(&command, None).await;

        assert_eq!(result.exit_code, 0);

        let output = result
            .files
            .iter()
            .find(|file| file.path == PathBuf::from("/result.txt"))
            .expect("python output file");
        assert_eq!(output.data, b"value=important\n");
        assert_eq!(output.origin, SandboxFileOrigin::Sandbox);
    }

    #[tokio::test]
    async fn execute_supports_ab_alias_for_agent_browser() {
        let tmp = std::env::temp_dir().join(format!(
            "oatmeal-ab-alias-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let script_path = tmp.join("agent-browser");
        std::fs::write(&script_path, "#!/bin/sh\nprintf '%s %s\\n' \"$1\" \"$2\"\n").unwrap();
        let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).unwrap();

        let executor = BashkitExecutor::new(script_path);
        let command = prepare_command("ab screenshot /tmp/out.png").unwrap();
        let result = executor.execute(&command, None).await;

        assert_eq!(result.exit_code, 0);
        let expected_path = std::env::temp_dir().join("tmp/out.png");
        assert_eq!(
            result.stdout,
            format!("screenshot {}\n", expected_path.display())
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn execute_returns_after_direct_process_exits_even_if_child_keeps_output_open() {
        let tmp = std::env::temp_dir().join(format!(
            "oatmeal-daemon-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let script_path = tmp.join("agent-browser");
        std::fs::write(
            &script_path,
            "#!/bin/sh\nprintf 'ready\\n'\n(sleep 2) &\nexit 0\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).unwrap();

        let executor = BashkitExecutor::new(script_path.clone());
        let command = prepare_command("agent-browser open https://example.com").unwrap();

        let result =
            tokio::time::timeout(Duration::from_millis(800), executor.execute(&command, None))
                .await
                .expect("daemon-style command should return after direct process exits");

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "ready\n");

        std::fs::remove_dir_all(&tmp).ok();
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

    #[test]
    fn mcp_path_policy_rewrites_windows_screenshot_output_into_cache_root() {
        let hints = Arc::new(Mutex::new(Vec::new()));
        let args = vec![
            "screenshot".to_string(),
            "https://github.com".to_string(),
            r"C:\Users\sep\.cache\sample.png".to_string(),
        ];

        let rewritten = rewrite_agent_browser_args(
            &args,
            &AgentBrowserPathPolicy::McpCacheRooted(PathBuf::from("/home/sep/.cache")),
            &hints,
        )
        .unwrap();

        assert_eq!(
            rewritten[2],
            "/home/sep/.cache/C/Users/sep/.cache/sample.png"
        );
        assert_eq!(
            hints.lock().unwrap()[0],
            PathBuf::from("/home/sep/.cache/C/Users/sep/.cache/sample.png")
        );
    }

    #[test]
    fn mcp_path_policy_rewrites_rooted_unix_screenshot_output_into_cache_root() {
        let hints = Arc::new(Mutex::new(Vec::new()));
        let args = vec![
            "screenshot".to_string(),
            "https://github.com".to_string(),
            "/tmp/sample.png".to_string(),
        ];

        let rewritten = rewrite_agent_browser_args(
            &args,
            &AgentBrowserPathPolicy::McpCacheRooted(PathBuf::from("/home/sep/.cache")),
            &hints,
        )
        .unwrap();

        assert_eq!(rewritten[2], "/home/sep/.cache/tmp/sample.png");
        assert_eq!(
            hints.lock().unwrap()[0],
            PathBuf::from("/home/sep/.cache/tmp/sample.png")
        );
    }

    #[test]
    fn mcp_path_policy_rewrites_windows_pdf_output_into_cache_root() {
        let hints = Arc::new(Mutex::new(Vec::new()));
        let args = vec![
            "pdf".to_string(),
            "https://github.com".to_string(),
            r"C:\Users\sep\.cache\sample.pdf".to_string(),
        ];

        let rewritten = rewrite_agent_browser_args(
            &args,
            &AgentBrowserPathPolicy::McpCacheRooted(PathBuf::from("/home/sep/.cache")),
            &hints,
        )
        .unwrap();

        assert_eq!(
            rewritten[2],
            "/home/sep/.cache/C/Users/sep/.cache/sample.pdf"
        );
        assert_eq!(
            hints.lock().unwrap()[0],
            PathBuf::from("/home/sep/.cache/C/Users/sep/.cache/sample.pdf")
        );
    }

    #[test]
    fn mcp_path_policy_rewrites_relative_pdf_output_into_cache_root() {
        let hints = Arc::new(Mutex::new(Vec::new()));
        let args = vec![
            "pdf".to_string(),
            "https://github.com".to_string(),
            "sample.pdf".to_string(),
        ];

        let rewritten = rewrite_agent_browser_args(
            &args,
            &AgentBrowserPathPolicy::McpCacheRooted(PathBuf::from("/home/sep/.cache")),
            &hints,
        )
        .unwrap();

        assert_eq!(rewritten[2], "/home/sep/.cache/sample.pdf");
        assert_eq!(
            hints.lock().unwrap()[0],
            PathBuf::from("/home/sep/.cache/sample.pdf")
        );
    }
}
