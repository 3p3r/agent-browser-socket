use crate::configuration::PageAgentConfig;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub struct EvalOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub async fn run_eval_script(
    binary_path: &Path,
    script: String,
    command_env: Option<&HashMap<String, String>>,
) -> Result<EvalOutput, String> {
    let mut command = Command::new(binary_path);
    command
        .arg("eval")
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AGENT_BROWSER_SESSION", "safe");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    if let Some(binary_dir) = binary_path.parent() {
        command.env("AGENT_BROWSER_HOME", binary_dir);
    }

    if let Some(env) = command_env {
        command.envs(env);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn page-agent eval: {error}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to open stdin pipe for page-agent eval".to_string())?;
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|error| format!("failed to write eval script to stdin: {error}"))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|error| format!("failed to wait for page-agent eval: {error}"))?;

    Ok(EvalOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

pub async fn run_page_agent_injection(
    binary_path: &Path,
    page_agent_config: &PageAgentConfig,
    command_env: Option<&HashMap<String, String>>,
) -> Result<i32, String> {
    let bundle = crate::server::render_page_agent_bundle(page_agent_config);
    let serialized_bundle = serde_json::to_string(&bundle).unwrap_or_else(|_| "\"\"".to_string());

    let injection_script = format!(
        r#"(async () => {{
    if (window.PageAgent) return 'already_loaded';

    const deadline = Date.now() + 10000;
    while (document.readyState === 'loading') {{
        if (Date.now() > deadline) throw new Error('page did not reach interactive within 10s');
        await new Promise(r => setTimeout(r, 100));
    }}

    const url = location.href;
    (0, eval)({serialized_bundle});

    await new Promise(r => setTimeout(r, 0));
    if (location.href !== url) throw new Error('page navigated during injection');
    if (!window.PageAgent) throw new Error('PageAgent not found on window after eval');
    return 'loaded';
}})()"#
    );

    const MAX_ATTEMPTS: u32 = 3;
    const RETRY_DELAY_MS: u64 = 500;

    for attempt in 1..=MAX_ATTEMPTS {
        let result = run_eval_script(binary_path, injection_script.clone(), command_env).await?;

        if result.exit_code == 0 {
            return Ok(0);
        }

        if attempt < MAX_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_millis(
                RETRY_DELAY_MS * attempt as u64,
            ))
            .await;
        }
    }

    Ok(1)
}

pub async fn run_page_agent_prompt(
    binary_path: &Path,
    prompt: &str,
    command_env: Option<&HashMap<String, String>>,
) -> Result<EvalOutput, String> {
    let script = crate::server::build_page_agent_prompt_script(prompt);
    run_eval_script(binary_path, script, command_env).await
}
