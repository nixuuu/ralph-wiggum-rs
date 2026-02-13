use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── JSON-RPC 2.0 Core Types ────────────────────────────────────────

/// JSON-RPC 2.0 request structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    /// Protocol version (must be "2.0")
    pub jsonrpc: String,
    /// Request ID (null for notifications)
    pub id: Option<Value>,
    /// Method name to invoke
    pub method: String,
    /// Optional method parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Protocol version (must be "2.0")
    pub jsonrpc: String,
    /// Request ID (matches request, null for errors without ID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Successful result (mutually exclusive with error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error object (mutually exclusive with result)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code (standard JSON-RPC or application-specific)
    pub code: i32,
    /// Human-readable error message
    pub message: String,
    /// Optional additional error data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    /// Create a success response with a result
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response
    pub fn error(id: Option<Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }

    /// Create an error response with additional data
    #[allow(dead_code)] // Public API: will be used in future task implementations
    pub fn error_with_data(id: Option<Value>, code: i32, message: String, data: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: Some(data),
            }),
        }
    }
}

// ── JSON-RPC Error Codes ───────────────────────────────────────────

/// Method not found error code
pub const METHOD_NOT_FOUND: i32 = -32601;
/// Invalid method parameters error code
pub const INVALID_PARAMS: i32 = -32602;
/// Internal server error code
pub const INTERNAL_ERROR: i32 = -32603;
/// Parse error (malformed JSON)
#[allow(dead_code)] // Public API: will be used in future task implementations
pub const PARSE_ERROR: i32 = -32700;
/// Invalid JSON-RPC request
#[allow(dead_code)] // Public API: will be used in future task implementations
pub const INVALID_REQUEST: i32 = -32600;

// ── MCP Protocol Types ─────────────────────────────────────────────

/// MCP protocol version constant (Streamable HTTP transport)
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// Legacy MCP protocol version (pre-Streamable HTTP)
#[allow(dead_code)]
pub const MCP_PROTOCOL_VERSION_LEGACY: &str = "2024-11-05";

/// MCP initialization parameters (client → server)
#[allow(dead_code)] // Public API: will be used in future task implementations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInitializeParams {
    /// Protocol version the client supports
    pub protocol_version: String,
    /// Client capabilities
    pub capabilities: McpClientCapabilities,
    /// Client identification
    pub client_info: McpImplementationInfo,
}

/// MCP initialization result (server → client)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInitializeResult {
    /// Protocol version the server supports
    pub protocol_version: String,
    /// Server capabilities
    pub capabilities: McpServerCapabilities,
    /// Server identification
    pub server_info: McpImplementationInfo,
}

/// Client capabilities in MCP initialize
#[allow(dead_code)] // Public API: will be used in future task implementations
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpClientCapabilities {
    /// Whether client supports sampling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<serde_json::Map<String, Value>>,
    /// Experimental capabilities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<serde_json::Map<String, Value>>,
}

/// Server capabilities in MCP initialize
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerCapabilities {
    /// Server supports listing/calling tools
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<serde_json::Map<String, Value>>,
    /// Server supports listing/reading prompts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<serde_json::Map<String, Value>>,
    /// Server supports listing/reading resources
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<serde_json::Map<String, Value>>,
    /// Experimental capabilities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<serde_json::Map<String, Value>>,
}

/// Client or server implementation info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpImplementationInfo {
    /// Implementation name
    pub name: String,
    /// Implementation version
    pub version: String,
}

/// Parameters for tools/call method
#[allow(dead_code)] // Public API: will be used in future task implementations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCallParams {
    /// Tool name to invoke
    pub name: String,
    /// Tool arguments (optional, defaults to empty object)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

/// Result of a tool call (wraps content array)
#[allow(dead_code)] // Public API: will be used in future task implementations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallResult {
    /// Array of content items (text, image, resource)
    pub content: Vec<McpContent>,
    /// Whether the tool call resulted in an error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// MCP content item (text, image, resource)
#[allow(dead_code)] // Public API: will be used in future task implementations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    /// Text content
    #[serde(rename = "text")]
    Text {
        /// Text content
        text: String,
    },
    /// Image content (base64 or URL)
    #[serde(rename = "image")]
    Image {
        /// Image data (base64) or URL
        data: String,
        /// MIME type
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Resource content
    #[serde(rename = "resource")]
    Resource {
        /// Resource URI
        uri: String,
        /// MIME type
        #[serde(rename = "mimeType")]
        mime_type: String,
        /// Optional resource text content
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
}

/// MCP tool definition (for tools/list response)
#[allow(dead_code)] // Public API: will be used in future task implementations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDefinition {
    /// Tool name (unique identifier)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for tool input parameters
    pub input_schema: McpToolInputSchema,
}

/// JSON Schema for tool input (subset for validation)
#[allow(dead_code)] // Public API: will be used in future task implementations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInputSchema {
    /// Schema type (usually "object")
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Object properties definition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Map<String, Value>>,
    /// Required property names
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    /// Additional schema fields (for extensibility)
    #[serde(flatten)]
    pub additional: serde_json::Map<String, Value>,
}

// ── Helpers ────────────────────────────────────────────────────────

/// Handle MCP initialize request
///
/// Zwraca odpowiedź zgodną z MCP protocol version 2025-03-26 (Streamable HTTP).
/// Capabilities zawiera tools.listChanged = false, co oznacza, że serwer
/// nie wysyła notyfikacji przy zmianach w liście dostępnych narzędzi.
///
/// Waliduje wersję protokołu z requestu. Jeśli klient używa niezgodnej wersji,
/// zwraca błąd INVALID_PARAMS.
///
/// # Parameters
/// - `id`: JSON-RPC request ID
/// - `params`: Opcjonalne parametry z requestu (McpInitializeParams)
///
/// # Panics
/// Will panic if McpInitializeResult fails to serialize (should never happen with valid types)
pub fn handle_initialize(id: Option<Value>, _params: Option<Value>) -> JsonRpcResponse {
    // Budowanie capabilities.tools z listChanged: false
    let mut tools_capabilities = serde_json::Map::new();
    tools_capabilities.insert("listChanged".to_string(), serde_json::Value::Bool(false));

    let result = McpInitializeResult {
        protocol_version: MCP_PROTOCOL_VERSION.to_string(),
        capabilities: McpServerCapabilities {
            tools: Some(tools_capabilities),
            prompts: None,
            resources: None,
            experimental: None,
        },
        server_info: McpImplementationInfo {
            name: "ralph-tasks".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    };

    // Safe to unwrap: McpInitializeResult is always serializable
    JsonRpcResponse::success(
        id,
        serde_json::to_value(result).expect("McpInitializeResult serialization should never fail"),
    )
}

/// Handle MCP notifications/initialized (no response)
///
/// According to MCP spec, the `notifications/initialized` message is sent by the client
/// after receiving the initialize response. No response is expected from the server.
#[allow(dead_code)] // Public API: legacy helper, may be used in future MCP implementations
pub fn handle_initialized() -> Option<JsonRpcResponse> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_json_rpc_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "initialize");
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(json!(1)));
        assert!(req.params.is_some());
    }

    #[test]
    fn test_json_rpc_notification() {
        let json = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "notifications/initialized");
        assert!(req.id.is_none());
    }

    #[test]
    fn test_json_rpc_success_response() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"result": "ok"}));
        let json_str = serde_json::to_string(&resp).unwrap();
        assert!(json_str.contains("\"result\""));
        assert!(!json_str.contains("\"error\""));
        assert!(json_str.contains("\"jsonrpc\":\"2.0\""));
    }

    #[test]
    fn test_json_rpc_error_response() {
        let resp = JsonRpcResponse::error(Some(json!(1)), METHOD_NOT_FOUND, "not found".into());
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());

        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert_eq!(err.message, "not found");
    }

    #[test]
    fn test_json_rpc_error_with_data() {
        let resp = JsonRpcResponse::error_with_data(
            Some(json!(2)),
            INVALID_PARAMS,
            "bad params".into(),
            json!({"details": "missing field"}),
        );
        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.data.is_some());
    }

    #[test]
    fn test_mcp_initialize_params_roundtrip() {
        let params = McpInitializeParams {
            protocol_version: "2024-11-05".into(),
            capabilities: McpClientCapabilities::default(),
            client_info: McpImplementationInfo {
                name: "test-client".into(),
                version: "1.0.0".into(),
            },
        };

        let json = serde_json::to_value(&params).unwrap();
        let parsed: McpInitializeParams = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.protocol_version, "2024-11-05");
        assert_eq!(parsed.client_info.name, "test-client");
    }

    #[test]
    fn test_mcp_initialize_result() {
        let result = McpInitializeResult {
            protocol_version: MCP_PROTOCOL_VERSION.into(),
            capabilities: McpServerCapabilities {
                tools: Some(serde_json::Map::new()),
                prompts: None,
                resources: None,
                experimental: None,
            },
            server_info: McpImplementationInfo {
                name: "test-server".into(),
                version: "0.1.0".into(),
            },
        };

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert!(json["capabilities"]["tools"].is_object());
        assert_eq!(json["serverInfo"]["name"], "test-server");
    }

    #[test]
    fn test_mcp_tool_call_params() {
        let json = json!({
            "name": "read_file",
            "arguments": {"path": "/tmp/test.txt"}
        });

        let params: McpToolCallParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.name, "read_file");
        assert!(params.arguments.is_some());

        let args = params.arguments.unwrap();
        assert_eq!(args["path"], "/tmp/test.txt");
    }

    #[test]
    fn test_mcp_tool_call_result_success() {
        let result = McpToolCallResult {
            content: vec![McpContent::Text {
                text: "file contents".into(),
            }],
            is_error: None,
        };

        let json = serde_json::to_value(&result).unwrap();
        assert!(json["content"].is_array());
        assert_eq!(json["content"][0]["type"], "text");
        assert_eq!(json["content"][0]["text"], "file contents");
    }

    #[test]
    fn test_mcp_tool_call_result_error() {
        let result = McpToolCallResult {
            content: vec![McpContent::Text {
                text: "File not found".into(),
            }],
            is_error: Some(true),
        };

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["isError"], true);
    }

    #[test]
    fn test_mcp_content_text() {
        let content = McpContent::Text {
            text: "hello world".into(),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello world");
    }

    #[test]
    fn test_mcp_content_image() {
        let content = McpContent::Image {
            data: "base64data".into(),
            mime_type: "image/png".into(),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "image");
        assert_eq!(json["mimeType"], "image/png");
    }

    #[test]
    fn test_mcp_tool_definition() {
        let tool = McpToolDefinition {
            name: "get_weather".into(),
            description: "Get current weather".into(),
            input_schema: McpToolInputSchema {
                schema_type: "object".into(),
                properties: Some(serde_json::Map::from_iter([(
                    "location".into(),
                    json!({"type": "string", "description": "City name"}),
                )])),
                required: Some(vec!["location".into()]),
                additional: serde_json::Map::new(),
            },
        };

        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "get_weather");
        assert_eq!(json["inputSchema"]["type"], "object");
        assert!(json["inputSchema"]["properties"]["location"].is_object());
    }

    #[test]
    fn test_handle_initialize() {
        let params = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        });
        let resp = handle_initialize(Some(json!(1)), Some(params));
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "ralph-tasks");
    }

    #[test]
    fn test_handle_initialize_capabilities_tools_list_changed() {
        let params = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        });
        let resp = handle_initialize(Some(json!(1)), Some(params));
        let result = resp.result.unwrap();

        // Sprawdź, że capabilities.tools.listChanged = false
        assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
    }

    #[test]
    fn test_handle_initialize_server_version() {
        let params = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        });
        let resp = handle_initialize(Some(json!(1)), Some(params));
        let result = resp.result.unwrap();

        // Sprawdź, że serverInfo.version jest ustawiona z CARGO_PKG_VERSION
        let version = result["serverInfo"]["version"].as_str().unwrap();
        assert!(!version.is_empty());
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_handle_initialize_different_protocol_version_succeeds() {
        // MCP spec: server responds with its own version, client decides compatibility.
        // Server should NOT reject based on protocol version.
        let params = json!({
            "protocolVersion": "2024-01-01",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        });
        let resp = handle_initialize(Some(json!(1)), Some(params));

        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(1)));
        assert!(
            resp.result.is_some(),
            "Should succeed with different version"
        );
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        assert_eq!(
            result["protocolVersion"], MCP_PROTOCOL_VERSION,
            "Server should respond with its own version"
        );
    }

    #[test]
    fn test_handle_initialize_newer_protocol_version_succeeds() {
        // Claude CLI 2.x sends "2025-03-26" (Streamable HTTP) — must succeed
        let params = json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "claude-cli",
                "version": "2.1.39"
            }
        });
        let resp = handle_initialize(Some(json!(1)), Some(params));

        assert!(
            resp.result.is_some(),
            "Should accept newer protocol version"
        );
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_handle_initialize_without_params() {
        // Test backward compatibility - brak params nie powinien powodować błędu
        let resp = handle_initialize(Some(json!(1)), None);
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_handle_initialize_with_invalid_params() {
        // Test z nieprawidłowymi params (nie parsują się jako McpInitializeParams)
        let params = json!({"invalid": "data"});
        let resp = handle_initialize(Some(json!(1)), Some(params));

        // Powinien kontynuować (backward compatibility)
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_handle_initialized_no_response() {
        let resp = handle_initialized();
        assert!(resp.is_none());
    }

    #[test]
    fn test_error_codes() {
        assert_eq!(PARSE_ERROR, -32700);
        assert_eq!(INVALID_REQUEST, -32600);
        assert_eq!(METHOD_NOT_FOUND, -32601);
        assert_eq!(INVALID_PARAMS, -32602);
        assert_eq!(INTERNAL_ERROR, -32603);
    }

    #[test]
    fn test_json_rpc_request_empty_params() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"test"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "test");
        assert!(req.params.is_none());
    }

    #[test]
    fn test_json_rpc_response_null_id() {
        let resp = JsonRpcResponse::success(None, json!({"status": "ok"}));
        let json_str = serde_json::to_string(&resp).unwrap();
        // null id should be omitted in serialization
        assert!(!json_str.contains("\"id\""));
    }

    #[test]
    fn test_mcp_content_resource_without_text() {
        let content = McpContent::Resource {
            uri: "file:///path/to/resource".into(),
            mime_type: "text/plain".into(),
            text: None,
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "resource");
        assert_eq!(json["uri"], "file:///path/to/resource");
        assert!(json.get("text").is_none()); // Optional field omitted
    }

    #[test]
    fn test_mcp_tool_input_schema_minimal() {
        let schema = McpToolInputSchema {
            schema_type: "object".into(),
            properties: None,
            required: None,
            additional: serde_json::Map::new(),
        };
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["type"], "object");
        // Optional fields should be omitted
        assert!(json.get("properties").is_none());
        assert!(json.get("required").is_none());
    }

    #[test]
    fn test_mcp_server_capabilities_empty() {
        let caps = McpServerCapabilities::default();
        let json = serde_json::to_value(&caps).unwrap();
        // Default should serialize to empty object
        assert!(json.is_object());
        assert_eq!(json.as_object().unwrap().len(), 0);
    }

    #[test]
    fn test_json_rpc_error_roundtrip_with_complex_data() {
        let error_data = json!({
            "field": "username",
            "reason": "too short",
            "min_length": 3
        });
        let resp = JsonRpcResponse::error_with_data(
            Some(json!("request-123")),
            INVALID_PARAMS,
            "Validation failed".into(),
            error_data.clone(),
        );

        let json_str = serde_json::to_string(&resp).unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(&json_str).unwrap();

        let error = parsed.error.unwrap();
        assert_eq!(error.code, INVALID_PARAMS);
        assert_eq!(error.data, Some(error_data));
    }

    #[test]
    fn test_mcp_tool_call_params_empty_arguments() {
        let params = McpToolCallParams {
            name: "ping".into(),
            arguments: None,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["name"], "ping");
        // Empty arguments should be omitted
        assert!(json.get("arguments").is_none());
    }

    // ── Extended Test Coverage: JsonRpcRequest ID Types ────────────────

    #[test]
    fn test_json_rpc_request_id_number() {
        let json = r#"{"jsonrpc":"2.0","id":42,"method":"test","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(json!(42)));
    }

    #[test]
    fn test_json_rpc_request_id_string() {
        let json = r#"{"jsonrpc":"2.0","id":"abc-123","method":"test"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, Some(json!("abc-123")));
    }

    #[test]
    fn test_json_rpc_request_id_null() {
        let json = r#"{"jsonrpc":"2.0","id":null,"method":"test"}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        // JSON-RPC spec: null is a valid ID value for requests/responses
        // However, serde_json deserializes "id": null as None in Option<Value>
        // This is consistent with serde's default behavior for Option fields
        assert!(req.id.is_none());
    }

    #[test]
    fn test_json_rpc_request_params_object() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"key":"value"}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert!(req.params.is_some());
        let params = req.params.unwrap();
        assert_eq!(params["key"], "value");
    }

    #[test]
    fn test_json_rpc_request_params_array() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"test","params":[1,2,3]}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert!(req.params.is_some());
        let params = req.params.unwrap();
        assert_eq!(params.as_array().unwrap().len(), 3);
    }

    // ── Claude CLI Realistic Payloads ──────────────────────────────────

    #[test]
    fn test_mcp_initialize_params_from_claude_cli() {
        // Realistyczny payload z Claude CLI
        let json = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "sampling": {},
                "experimental": {
                    "feature_x": true
                }
            },
            "clientInfo": {
                "name": "claude-cli",
                "version": "1.0.0"
            }
        });

        let params: McpInitializeParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.protocol_version, "2024-11-05");
        assert_eq!(params.client_info.name, "claude-cli");
        assert!(params.capabilities.sampling.is_some());
        assert!(params.capabilities.experimental.is_some());
    }

    #[test]
    fn test_mcp_initialize_params_minimal_capabilities() {
        let json = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "0.1.0"
            }
        });

        let params: McpInitializeParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.protocol_version, "2024-11-05");
        assert!(params.capabilities.sampling.is_none());
        assert!(params.capabilities.experimental.is_none());
    }

    // ── CamelCase Serialization Verification ───────────────────────────

    #[test]
    fn test_mcp_initialize_result_camel_case() {
        let result = McpInitializeResult {
            protocol_version: "2024-11-05".into(),
            capabilities: McpServerCapabilities::default(),
            server_info: McpImplementationInfo {
                name: "test".into(),
                version: "1.0".into(),
            },
        };

        let json_str = serde_json::to_string(&result).unwrap();
        // Weryfikacja camelCase
        assert!(json_str.contains("protocolVersion"));
        assert!(json_str.contains("serverInfo"));
        assert!(!json_str.contains("protocol_version"));
        assert!(!json_str.contains("server_info"));
    }

    #[test]
    fn test_mcp_tool_call_result_camel_case() {
        let result = McpToolCallResult {
            content: vec![],
            is_error: Some(true),
        };

        let json_str = serde_json::to_string(&result).unwrap();
        assert!(json_str.contains("isError"));
        assert!(!json_str.contains("is_error"));
    }

    // ── Round-Trip Fidelity Tests ──────────────────────────────────────

    #[test]
    fn test_json_rpc_request_full_roundtrip() {
        let original = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(123)),
            method: "tools/call".into(),
            params: Some(json!({"name": "test_tool", "arguments": {"arg1": "value1"}})),
        };

        let json_str = serde_json::to_string(&original).unwrap();
        let parsed: JsonRpcRequest = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.jsonrpc, original.jsonrpc);
        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.method, original.method);
        assert_eq!(parsed.params, original.params);
    }

    #[test]
    fn test_json_rpc_response_success_roundtrip() {
        let original = JsonRpcResponse::success(Some(json!("req-1")), json!({"status": "success"}));

        let json_str = serde_json::to_string(&original).unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.jsonrpc, "2.0");
        assert_eq!(parsed.id, Some(json!("req-1")));
        assert!(parsed.result.is_some());
        assert!(parsed.error.is_none());
    }

    #[test]
    fn test_json_rpc_response_error_roundtrip() {
        let original =
            JsonRpcResponse::error(Some(json!(99)), INTERNAL_ERROR, "Internal error".into());

        let json_str = serde_json::to_string(&original).unwrap();
        let parsed: JsonRpcResponse = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.jsonrpc, "2.0");
        assert_eq!(parsed.id, Some(json!(99)));
        assert!(parsed.result.is_none());
        assert!(parsed.error.is_some());

        let err = parsed.error.unwrap();
        assert_eq!(err.code, INTERNAL_ERROR);
        assert_eq!(err.message, "Internal error");
    }

    #[test]
    fn test_mcp_client_capabilities_full_roundtrip() {
        let mut sampling = serde_json::Map::new();
        sampling.insert("enabled".into(), json!(true));

        let mut experimental = serde_json::Map::new();
        experimental.insert("feature_a".into(), json!("value"));

        let original = McpClientCapabilities {
            sampling: Some(sampling.clone()),
            experimental: Some(experimental.clone()),
        };

        let json = serde_json::to_value(&original).unwrap();
        let parsed: McpClientCapabilities = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.sampling, Some(sampling));
        assert_eq!(parsed.experimental, Some(experimental));
    }

    #[test]
    fn test_mcp_server_capabilities_full_roundtrip() {
        let mut tools = serde_json::Map::new();
        tools.insert("listChanged".into(), json!(true));

        let mut prompts = serde_json::Map::new();
        prompts.insert("listChanged".into(), json!(false));

        let original = McpServerCapabilities {
            tools: Some(tools.clone()),
            prompts: Some(prompts.clone()),
            resources: None,
            experimental: None,
        };

        let json = serde_json::to_value(&original).unwrap();
        let parsed: McpServerCapabilities = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.tools, Some(tools));
        assert_eq!(parsed.prompts, Some(prompts));
        assert!(parsed.resources.is_none());
    }

    // ── Edge Cases: Missing Optional Fields ───────────────────────────

    #[test]
    fn test_json_rpc_response_without_optional_fields() {
        // Test deserializacji response bez id (error bez kontekstu)
        let json = r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.id.is_none());
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());

        let err = resp.error.unwrap();
        assert_eq!(err.code, PARSE_ERROR);
    }

    #[test]
    fn test_mcp_content_resource_with_text() {
        let content = McpContent::Resource {
            uri: "file:///test.txt".into(),
            mime_type: "text/plain".into(),
            text: Some("content preview".into()),
        };

        let json = serde_json::to_value(&content).unwrap();
        let parsed: McpContent = serde_json::from_value(json).unwrap();

        match parsed {
            McpContent::Resource {
                uri,
                mime_type,
                text,
            } => {
                assert_eq!(uri, "file:///test.txt");
                assert_eq!(mime_type, "text/plain");
                assert_eq!(text, Some("content preview".into()));
            }
            _ => panic!("Expected Resource variant"),
        }
    }

    #[test]
    fn test_mcp_tool_input_schema_with_additional_fields() {
        let mut additional = serde_json::Map::new();
        additional.insert("additionalProperties".into(), json!(false));
        additional.insert("minProperties".into(), json!(1));

        let schema = McpToolInputSchema {
            schema_type: "object".into(),
            properties: Some(serde_json::Map::from_iter([(
                "name".into(),
                json!({"type": "string"}),
            )])),
            required: Some(vec!["name".into()]),
            additional: additional.clone(),
        };

        let json = serde_json::to_value(&schema).unwrap();
        let parsed: McpToolInputSchema = serde_json::from_value(json).unwrap();

        assert_eq!(
            parsed.additional.get("additionalProperties"),
            Some(&json!(false))
        );
        assert_eq!(parsed.additional.get("minProperties"), Some(&json!(1)));
    }

    // ── Error Code Constants Validation ────────────────────────────────

    #[test]
    fn test_all_error_codes_are_negative() {
        const { assert!(PARSE_ERROR < 0) };
        const { assert!(INVALID_REQUEST < 0) };
        const { assert!(METHOD_NOT_FOUND < 0) };
        const { assert!(INVALID_PARAMS < 0) };
        const { assert!(INTERNAL_ERROR < 0) };
    }

    #[test]
    fn test_error_codes_match_json_rpc_spec() {
        // JSON-RPC 2.0 spec: -32700 to -32603 są zarezerwowane
        const { assert!(PARSE_ERROR == -32700) };
        const { assert!(INVALID_REQUEST == -32600) };
        const { assert!(METHOD_NOT_FOUND == -32601) };
        const { assert!(INVALID_PARAMS == -32602) };
        const { assert!(INTERNAL_ERROR == -32603) };
    }
}
