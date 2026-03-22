use crate::configuration::PageAgentConfig;
use once_cell::sync::Lazy;
use secure_string::SecureString;
use std::error::Error as StdError;

pub static URI_SCHEME: Lazy<SecureString> = Lazy::new(|| SecureString::from("oatmeal"));
const PAGE_AGENT_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/page-agent.demo.sanitized.js"));

pub fn render_page_agent_bundle(config: &PageAgentConfig) -> String {
    let with_model = replace_js_string_constant(PAGE_AGENT_JS, "DEMO_MODEL", &config.model);
    let with_url = replace_js_string_constant(&with_model, "DEMO_BASE_URL", &config.url);
    replace_js_string_constant(&with_url, "DEMO_API_KEY", &config.key)
}

fn replace_js_string_constant(source: &str, constant_name: &str, replacement: &str) -> String {
    let replacement_value =
        serde_json::to_string(replacement).unwrap_or_else(|_| "\"\"".to_string());
    let needle = format!("{constant_name}=");

    let mut output = source.to_string();
    let mut search_start = 0;

    while let Some(relative_index) = output[search_start..].find(&needle) {
        let name_index = search_start + relative_index;
        let mut value_start = name_index + needle.len();

        while value_start < output.len()
            && output[value_start..]
                .chars()
                .next()
                .map(|ch| ch.is_whitespace())
                .unwrap_or(false)
        {
            value_start += output[value_start..]
                .chars()
                .next()
                .map(|ch| ch.len_utf8())
                .unwrap_or(1);
        }

        let quote_char = match output[value_start..].chars().next() {
            Some('"') => '"',
            Some('\'') => '\'',
            _ => {
                search_start = value_start;
                continue;
            }
        };

        let content_start = value_start + quote_char.len_utf8();
        let mut escaped = false;
        let mut closing_quote = None;

        for (offset, ch) in output[content_start..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote_char {
                closing_quote = Some(content_start + offset);
                break;
            }
        }

        let Some(closing_quote) = closing_quote else {
            break;
        };

        output.replace_range(value_start..=closing_quote, &replacement_value);
        search_start = value_start + replacement_value.len();
    }

    output
}

fn is_missing_uri_scheme_error(error: &(dyn StdError + 'static)) -> bool {
    if !cfg!(windows) {
        return false;
    }

    let message = error.to_string().to_ascii_lowercase();
    message.contains("failed to find the scheme key")
        || (message.contains("scheme") && message.contains("not found"))
}

pub fn unregister_uri_scheme() -> Result<bool, Box<dyn StdError>> {
    if !sysuri::is_registered(URI_SCHEME.unsecure())? {
        return Ok(false);
    }

    match sysuri::unregister(URI_SCHEME.unsecure()) {
        Ok(()) => Ok(true),
        Err(error) if is_missing_uri_scheme_error(&error) => Ok(false),
        Err(error) => Err(Box::new(error)),
    }
}

pub fn build_page_agent_prompt_script(prompt: &str) -> String {
    let serialized_prompt = serde_json::to_string(prompt).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(() => {{
    const input = document.querySelector('#page-agent-runtime_agent-panel input');
    if (!input) throw new Error('page-agent input not found');

    input.focus();
    input.value = {serialized_prompt};
    input.dispatchEvent(new Event('input', {{ bubbles: true }}));

    const keyOptions = {{
        key: 'Enter',
        code: 'Enter',
        keyCode: 13,
        which: 13,
        bubbles: true,
        cancelable: true,
        composed: true
    }};
    input.dispatchEvent(new KeyboardEvent('keydown', keyOptions));
    input.dispatchEvent(new KeyboardEvent('keypress', keyOptions));
    input.dispatchEvent(new KeyboardEvent('keyup', keyOptions));

    return 'submitted';
}})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_page_agent_bundle_replaces_demo_constants_and_url() {
        let config = PageAgentConfig {
            model: "my-model".to_string(),
            url: "http://localhost:11434/v1".to_string(),
            key: "my-key".to_string(),
        };

        let rendered = render_page_agent_bundle(&config);

        assert!(rendered.contains("DEMO_MODEL=\"my-model\""));
        assert!(rendered.contains("DEMO_BASE_URL=\"http://localhost:11434/v1\""));
        assert!(rendered.contains("DEMO_API_KEY=\"my-key\""));
        assert!(!rendered.contains("DEMO_BASE_URL=\"https://"));
    }

    #[test]
    fn replace_js_string_constant_replaces_pattern_without_hardcoded_url() {
        let source =
            "const DEMO_MODEL=\"x\",DEMO_BASE_URL=\"https://example.invalid/demo\",DEMO_API_KEY=\"y\";";

        let replaced =
            replace_js_string_constant(source, "DEMO_BASE_URL", "http://localhost:11434/v1");

        assert!(replaced.contains("DEMO_BASE_URL=\"http://localhost:11434/v1\""));
        assert!(!replaced.contains("https://example.invalid/demo"));
    }
}
