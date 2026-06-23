use crate::parser::HttpRequest;
use base64::prelude::*;
use reqwest::{Client, Method, Request};
use std::collections::HashMap;
use std::str::FromStr;

fn process_auth_header(value: &str) -> String {
    if let Some(stripped) = value.strip_prefix("Basic ") {
        let remainder = stripped.trim();
        // If it contains a space, it's likely "username password"
        if let Some((user, pass)) = remainder.split_once(' ') {
            let auth = format!("{}:{}", user.trim(), pass.trim());
            let encoded = BASE64_STANDARD.encode(auth);
            return format!("Basic {}", encoded);
        }
    }
    value.to_string()
}

fn resolve_system_variables(text: &str) -> String {
    let mut resolved = text.to_string();

    // {{$guid}}
    while resolved.contains("{{$guid}}") {
        let guid = uuid::Uuid::new_v4().to_string();
        resolved = resolved.replace("{{$guid}}", &guid);
    }

    // {{$datetime rfc1123}}
    while resolved.contains("{{$datetime rfc1123}}") {
        let now = chrono::Utc::now().to_rfc2822();
        resolved = resolved.replace("{{$datetime rfc1123}}", &now);
    }

    // {{$datetime iso8601}}
    while resolved.contains("{{$datetime iso8601}}") {
        let now = chrono::Utc::now().to_rfc3339();
        resolved = resolved.replace("{{$datetime iso8601}}", &now);
    }

    // Default {{$datetime}} (iso8601)
    while resolved.contains("{{$datetime}}") {
        let now = chrono::Utc::now().to_rfc3339();
        resolved = resolved.replace("{{$datetime}}", &now);
    }

    // {{$randomInt min max}}
    while let Some(start_idx) = resolved.find("{{$randomInt") {
        let end_idx = resolved[start_idx..].find("}}");
        if let Some(offset) = end_idx {
            let full_match = &resolved[start_idx..start_idx + offset + 2];
            let parts: Vec<&str> = full_match
                .trim_start_matches("{{")
                .trim_end_matches("}}")
                .split_whitespace()
                .collect();

            let mut min = 0;
            let mut max = 1000;

            if parts.len() >= 3 {
                min = parts[1].parse().unwrap_or(0);
                max = parts[2].parse().unwrap_or(1000);
            } else if parts.len() == 2 {
                max = parts[1].parse().unwrap_or(1000);
            }

            if min > max {
                std::mem::swap(&mut min, &mut max);
            }

            // Using standard timestamp as poor man's random for now,
            // to avoid pulling in full `rand` crate just for this.
            // A more robust implementation would use `rand::Rng`.
            let time_val = chrono::Utc::now().timestamp_subsec_nanos() as i32;
            let range = (max - min).max(1);
            let random_val = min + (time_val.abs() % range);

            resolved = resolved.replace(full_match, &random_val.to_string());
        } else {
            break; // Malformed, prevent infinite loop
        }
    }

    resolved
}

fn resolve_variables(text: &str, variables: &HashMap<&str, &str>) -> String {
    let mut resolved = text.to_string();

    // 1. Resolve custom user variables from the file
    for (key, value) in variables {
        let placeholder = format!("{{{{{}}}}}", key);
        resolved = resolved.replace(&placeholder, value);
    }

    // 2. Resolve built-in system variables
    resolve_system_variables(&resolved)
}

/// Converts our parsed HttpRequest into a native reqwest::Request.
pub fn build_request(
    client: &Client,
    req: &HttpRequest<'_>,
    variables: &HashMap<&str, &str>,
) -> anyhow::Result<Request> {
    let method = Method::from_str(req.method)
        .map_err(|_| anyhow::anyhow!("Invalid HTTP Method: {}", req.method))?;

    let url = resolve_variables(req.url, variables);
    let mut request_builder = client.request(method, &url);

    for (key, value) in &req.headers {
        let resolved_key = resolve_variables(key, variables);
        let mut resolved_value = resolve_variables(value, variables);

        if resolved_key.to_lowercase() == "authorization" {
            resolved_value = process_auth_header(&resolved_value);
        }

        request_builder = request_builder.header(resolved_key, resolved_value);
    }

    if let Some(body_text) = req.body {
        let resolved_body = resolve_variables(body_text, variables);
        request_builder = request_builder.body(resolved_body);
    }

    let request = request_builder
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build request: {}", e))?;

    Ok(request)
}

/// Returns true if the `Content-Type` header denotes a JSON payload, including
/// structured-suffix types such as `application/problem+json`.
fn is_json_content_type(content_type: Option<&str>) -> bool {
    content_type
        .map(|ct| {
            let ct = ct.to_ascii_lowercase();
            ct.contains("application/json") || ct.contains("+json")
        })
        .unwrap_or(false)
}

/// Pretty-prints a JSON response body with a 2-space indent. If the response is
/// not JSON (per `content_type`) or the body fails to parse, it is returned
/// unchanged.
pub fn format_response_body(content_type: Option<&str>, body: &str) -> String {
    if is_json_content_type(content_type)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(body)
        && let Ok(pretty) = serde_json::to_string_pretty(&value)
    {
        return pretty;
    }
    body.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Method;

    #[test]
    fn test_build_request() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "POST",
            url: "https://httpbin.org/post",
            headers: vec![("Content-Type", "application/json"), ("X-Custom", "Test")],
            body: Some("{\"hello\":\"world\"}"),
        };

        let reqwest_req =
            build_request(&client, &http_req, &HashMap::new()).expect("Failed to build request");

        // Verify Method
        assert_eq!(reqwest_req.method(), Method::POST);

        // Verify URL
        assert_eq!(reqwest_req.url().as_str(), "https://httpbin.org/post");

        // Verify Headers
        let headers = reqwest_req.headers();
        assert_eq!(headers.get("Content-Type").unwrap(), "application/json");
        assert_eq!(headers.get("X-Custom").unwrap(), "Test");

        // Verify Body
        let body_bytes = reqwest_req.body().unwrap().as_bytes().unwrap();
        assert_eq!(body_bytes, b"{\"hello\":\"world\"}");
    }

    #[test]
    fn test_build_request_with_variables() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "{{baseUrl}}/api/{{userId}}",
            headers: vec![("Authorization", "Bearer {{token}}")],
            body: Some("{\"id\":\"{{userId}}\"}"),
        };

        let mut vars = HashMap::new();
        vars.insert("baseUrl", "https://api.example.com");
        vars.insert("userId", "123");
        vars.insert("token", "secret123");

        let reqwest_req =
            build_request(&client, &http_req, &vars).expect("Failed to build request");

        assert_eq!(
            reqwest_req.url().as_str(),
            "https://api.example.com/api/123"
        );
        assert_eq!(
            reqwest_req.headers().get("Authorization").unwrap(),
            "Bearer secret123"
        );
        let body_bytes = reqwest_req.body().unwrap().as_bytes().unwrap();
        assert_eq!(body_bytes, b"{\"id\":\"123\"}");
    }

    #[test]
    fn test_system_variables() {
        let vars = HashMap::new();

        let guid_text = resolve_variables("id: {{$guid}}", &vars);
        assert!(guid_text.starts_with("id: "));
        assert_eq!(guid_text.len(), 4 + 36); // "id: " + 36 char UUID

        let dt_iso = resolve_variables("time: {{$datetime iso8601}}", &vars);
        assert!(dt_iso.contains('T')); // ISO8601 has a 'T'
        assert!(dt_iso.contains('+') || dt_iso.contains('Z'));

        let rand_int = resolve_variables("number: {{$randomInt 10 20}}", &vars);
        let num_str = rand_int.strip_prefix("number: ").unwrap();
        let num: i32 = num_str.parse().unwrap();
        assert!((10..=20).contains(&num));
    }

    #[test]
    fn test_basic_auth_encoding() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "https://httpbin.org/basic-auth/user/passwd",
            headers: vec![("Authorization", "Basic user passwd")],
            body: None,
        };

        let reqwest_req =
            build_request(&client, &http_req, &HashMap::new()).expect("Failed to build request");

        let auth_header = reqwest_req
            .headers()
            .get("Authorization")
            .unwrap()
            .to_str()
            .unwrap();
        // "user:passwd" in base64 is "dXNlcjpwYXNzd2Q="
        assert_eq!(auth_header, "Basic dXNlcjpwYXNzd2Q=");
    }

    #[test]
    fn test_basic_auth_with_variables() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "https://httpbin.org/basic-auth/admin/secret",
            headers: vec![("Authorization", "Basic {{user}} {{pass}}")],
            body: None,
        };

        let mut vars = HashMap::new();
        vars.insert("user", "admin");
        vars.insert("pass", "secret");

        let reqwest_req =
            build_request(&client, &http_req, &vars).expect("Failed to build request");

        let auth_header = reqwest_req
            .headers()
            .get("Authorization")
            .unwrap()
            .to_str()
            .unwrap();
        // "admin:secret" in base64 is "YWRtaW46c2VjcmV0"
        assert_eq!(auth_header, "Basic YWRtaW46c2VjcmV0");
    }

    #[test]
    fn test_format_response_body_json() {
        let body = r#"{"name":"John","nested":{"a":1},"list":[1,2]}"#;
        let pretty = format_response_body(Some("application/json"), body);
        // Original key order is preserved (serde_json "preserve_order" feature).
        let expected = "{\n  \"name\": \"John\",\n  \"nested\": {\n    \"a\": 1\n  },\n  \"list\": [\n    1,\n    2\n  ]\n}";
        assert_eq!(pretty, expected);
    }

    #[test]
    fn test_format_response_body_json_with_charset() {
        let body = r#"{"ok":true}"#;
        let pretty = format_response_body(Some("application/json; charset=utf-8"), body);
        assert_eq!(pretty, "{\n  \"ok\": true\n}");
    }

    #[test]
    fn test_format_response_body_structured_suffix() {
        let body = r#"{"type":"about:blank"}"#;
        let pretty = format_response_body(Some("application/problem+json"), body);
        assert_eq!(pretty, "{\n  \"type\": \"about:blank\"\n}");
    }

    #[test]
    fn test_format_response_body_non_json_unchanged() {
        let body = "<html><body>hi</body></html>";
        assert_eq!(format_response_body(Some("text/html"), body), body);
        // No content type => unchanged.
        assert_eq!(format_response_body(None, body), body);
    }

    #[test]
    fn test_format_response_body_invalid_json_unchanged() {
        // Declared JSON but not parseable: keep the raw body rather than erroring.
        let body = "{not valid json";
        assert_eq!(format_response_body(Some("application/json"), body), body);
    }
}
