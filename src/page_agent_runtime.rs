use crate::configuration::PageAgentConfig;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
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
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(env) = command_env {
        command.envs(env);
    }

    let output = command
        .output()
        .await
        .map_err(|error| format!("failed to spawn page-agent eval: {error}"))?;

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
    let max_chunk_bytes = 20_000;

    let init = run_eval_script(
        binary_path,
        "window.__oatmealPageAgentChunks = [];".to_string(),
        command_env,
    )
    .await?;
    if init.exit_code != 0 {
        return Ok(init.exit_code);
    }

    let mut chunk_start = 0;
    while chunk_start < bundle.len() {
        let mut chunk_end = (chunk_start + max_chunk_bytes).min(bundle.len());
        while chunk_end > chunk_start && !bundle.is_char_boundary(chunk_end) {
            chunk_end -= 1;
        }

        if chunk_end == chunk_start {
            break;
        }

        let chunk = &bundle[chunk_start..chunk_end];
        let serialized_chunk = serde_json::to_string(chunk).unwrap_or_else(|_| "\"\"".to_string());
        let append_script = format!("window.__oatmealPageAgentChunks.push({serialized_chunk});");

        let append = run_eval_script(binary_path, append_script, command_env).await?;
        if append.exit_code != 0 {
            return Ok(append.exit_code);
        }

        chunk_start = chunk_end;
    }

    let finalize_script = r#"(() => {
    if (window.PageAgent) return 'already_loaded';
    const source = (window.__oatmealPageAgentChunks || []).join('');
    delete window.__oatmealPageAgentChunks;
    (0, eval)(source);
    if (!window.PageAgent) throw new Error('PageAgent not found on window after eval');
    return 'loaded';
})()"#;

    let finalize = run_eval_script(binary_path, finalize_script.to_string(), command_env).await?;
    Ok(finalize.exit_code)
}

pub async fn run_page_agent_prompt(
    binary_path: &Path,
    prompt: &str,
    command_env: Option<&HashMap<String, String>>,
) -> Result<EvalOutput, String> {
    let script = crate::server::build_page_agent_prompt_script(prompt);
    run_eval_script(binary_path, script, command_env).await
}
