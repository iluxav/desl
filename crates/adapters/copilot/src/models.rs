//! Copilot's model list, asked over the Agent Client Protocol (`copilot --acp`): `initialize`,
//! then `session/new`, whose answer lists the models and the reasoning levels. No prompt is
//! sent and nothing is billed; it answers in about a second. Verified against 1.0.112.

use desl_ir::adapter::{scrub_env, terminate_group};
use desl_ir::{Effort, ModelInfo};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const ANSWER_WITHIN: Duration = Duration::from_secs(15);

pub fn list(cwd: &Path, login_vars: &[&str]) -> Result<Vec<ModelInfo>, String> {
    let mut cmd = Command::new("copilot");
    cmd.arg("--acp");
    scrub_env(&mut cmd, login_vars);
    cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let (mut stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
        (Some(i), Some(o)) => (i, o),
        _ => return Err("its pipes did not open".into()),
    };
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let mut ask = |id: u64, method: &str, params: Value| -> Result<Value, String> {
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        stdin.write_all(format!("{request}\n").as_bytes()).and_then(|()| stdin.flush()).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + ANSWER_WITHIN;
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(v) if v["id"] == id => {
                    return match v.get("error") {
                        Some(e) => Err(e["message"].as_str().unwrap_or("it refused").to_string()),
                        None => Ok(v["result"].clone()),
                    };
                }
                // Notifications and its own requests to the client are not the answer.
                Ok(_) => {}
                Err(_) => return Err("it did not answer within fifteen seconds".into()),
            }
        }
    };
    let answer = ask(1, "initialize", json!({ "protocolVersion": 1, "clientCapabilities": {} }))
        .and_then(|_| ask(2, "session/new", json!({ "cwd": cwd.to_string_lossy(), "mcpServers": [] })));
    drop(stdin);
    terminate_group(&mut child, Duration::from_secs(2));
    let models = parse(&answer?);
    if models.is_empty() { Err("it listed none".into()) } else { Ok(models) }
}

/// The models in a `session/new` answer, in the order Copilot's own picker shows them. The
/// answer gives the reasoning levels of the model in use only, so every model is listed with
/// those; Copilot says so itself when a model does not take a level.
pub fn parse(result: &Value) -> Vec<ModelInfo> {
    let current = result["models"]["currentModelId"].as_str();
    let effort = result["configOptions"].as_array().into_iter().flatten().find(|o| o["id"] == "reasoning_effort");
    let efforts: Vec<Effort> = effort
        .and_then(|o| o["options"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(Effort { id: o["value"].as_str()?.to_string(), description: o["description"].as_str().unwrap_or("").to_string() })
        })
        .collect();
    let default_effort = effort.and_then(|o| o["currentValue"].as_str()).map(str::to_string);
    result["models"]["availableModels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m["modelId"].as_str()?.to_string();
            let default = current == Some(id.as_str());
            // "Versatile · Medium cost", and what a prompt costs against the plan: "1x".
            let description = match (m["description"].as_str().unwrap_or(""), m["_meta"]["copilotUsage"].as_str()) {
                (d, Some(usage)) if !d.is_empty() => format!("{d} · {usage}"),
                (d, _) => d.to_string(),
            };
            Some(ModelInfo {
                name: m["name"].as_str().unwrap_or(&id).to_string(),
                id,
                description,
                default,
                efforts: efforts.clone(),
                default_effort: default.then(|| default_effort.clone()).flatten(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_models_come_from_a_new_session_answer_with_the_one_in_use_marked() {
        let answer = json!({
            "sessionId": "c1e8777c-ee50-488b-b49e-2dc37f1e5ea2",
            "models": { "currentModelId": "gpt-6-luna", "availableModels": [
                { "modelId": "auto", "name": "Auto", "description": "Let Copilot pick the best model" },
                { "modelId": "gpt-6-luna", "name": "GPT-6 Luna", "description": "Lightweight · Low cost",
                  "_meta": { "copilotUsage": "1x", "copilotEnablement": "enabled" } },
                { "modelId": "claude-haiku-4.5", "name": "Claude Haiku 4.5", "description": "Lightweight · Low cost",
                  "_meta": { "copilotUsage": "0.33x" } }
            ] },
            "configOptions": [
                { "type": "select", "id": "model", "currentValue": "gpt-6-luna", "options": [] },
                { "type": "select", "id": "reasoning_effort", "currentValue": "medium",
                  "options": [{ "value": "low", "name": "Low" }, { "value": "medium", "name": "Medium" }, { "value": "high", "name": "High" }] }
            ]
        });
        let models = parse(&answer);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["auto", "gpt-6-luna", "claude-haiku-4.5"]);
        assert_eq!(models.iter().filter(|m| m.default).map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gpt-6-luna"]);
        assert_eq!(models[1].description, "Lightweight · Low cost · 1x");
        assert_eq!(models[1].default_effort.as_deref(), Some("medium"));
        assert_eq!(models[2].default_effort, None);
        assert_eq!(models[2].efforts.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["low", "medium", "high"]);
        assert!(parse(&json!({})).is_empty());
    }
}
