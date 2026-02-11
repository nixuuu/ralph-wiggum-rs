mod handlers;
mod protocol;
mod tools;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::shared::error::Result;

use protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND};

/// Run the MCP server on stdin/stdout (JSON-RPC 2.0).
pub fn execute(tasks_file: PathBuf) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let reader = stdin.lock();
    let mut writer = stdout.lock();

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(req) => handle_request(&req, &tasks_file),
            Err(e) => Some(JsonRpcResponse::error(
                None,
                -32700,
                format!("Parse error: {e}"),
            )),
        };

        if let Some(resp) = response {
            let json = serde_json::to_string(&resp).unwrap_or_else(|e| {
                serde_json::to_string(&JsonRpcResponse::error(
                    None,
                    INTERNAL_ERROR,
                    format!("Serialization error: {e}"),
                ))
                .unwrap()
            });
            writeln!(writer, "{json}")?;
            writer.flush()?;
        }
    }

    Ok(())
}

fn handle_request(req: &JsonRpcRequest, tasks_file: &Path) -> Option<JsonRpcResponse> {
    match req.method.as_str() {
        // MCP lifecycle
        "initialize" => Some(protocol::handle_initialize(req.id.clone())),
        "notifications/initialized" => protocol::handle_initialized(),

        // Tool listing
        "tools/list" => Some(JsonRpcResponse::success(
            req.id.clone(),
            tools::list_tools(),
        )),

        // Tool execution
        "tools/call" => {
            let params = req.params.as_ref();
            let tool_name = params
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str());
            let tool_args = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(json!({}));

            match tool_name {
                Some(name) => {
                    let result = tools::dispatch(name, &tool_args, tasks_file);
                    Some(match result {
                        Ok(value) => JsonRpcResponse::success(
                            req.id.clone(),
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": serde_json::to_string_pretty(&value).unwrap_or_default()
                                }]
                            }),
                        ),
                        Err(err) => JsonRpcResponse::success(
                            req.id.clone(),
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": err
                                }],
                                "isError": true
                            }),
                        ),
                    })
                }
                None => Some(JsonRpcResponse::error(
                    req.id.clone(),
                    INVALID_PARAMS,
                    "Missing tool name in tools/call".into(),
                )),
            }
        }

        // Notifications — no response
        method if method.starts_with("notifications/") => None,

        // Unknown method
        _ => Some(JsonRpcResponse::error(
            req.id.clone(),
            METHOD_NOT_FOUND,
            format!("Unknown method: {}", req.method),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_initialize() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "initialize".into(),
            params: Some(json!({"capabilities": {}})),
        };
        let resp = handle_request(&req, &PathBuf::from("test.yml")).unwrap();
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_handle_tools_list() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(2)),
            method: "tools/list".into(),
            params: None,
        };
        let resp = handle_request(&req, &PathBuf::from("test.yml")).unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["tools"].as_array().unwrap().len(), 11);
    }

    #[test]
    fn test_handle_unknown_method() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(3)),
            method: "unknown/method".into(),
            params: None,
        };
        let resp = handle_request(&req, &PathBuf::from("test.yml")).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[test]
    fn test_handle_notification_no_response() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: None,
            method: "notifications/initialized".into(),
            params: None,
        };
        let resp = handle_request(&req, &PathBuf::from("test.yml"));
        assert!(resp.is_none());
    }

    #[test]
    fn test_handle_tools_call_missing_name() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(4)),
            method: "tools/call".into(),
            params: Some(json!({"arguments": {}})),
        };
        let resp = handle_request(&req, &PathBuf::from("test.yml")).unwrap();
        assert!(resp.error.is_some());
    }
}
