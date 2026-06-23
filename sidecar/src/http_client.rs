use crate::parser::HttpRequest;
use base64::prelude::*;
use reqwest::{Client, Method, Request};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

/// Loads key/value pairs from the nearest `.env` file, searching from `base_dir`
/// upwards through its parent directories. The first `.env` found wins (the one
/// closest to the request file), mirroring how editors resolve dotenv files.
fn load_dotenv(base_dir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut dir = Some(base_dir);
    while let Some(d) = dir {
        let candidate = d.join(".env");
        if candidate.is_file() {
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                parse_dotenv_into(&content, &mut map);
            }
            break;
        }
        dir = d.parent();
    }
    map
}

/// Minimal `.env` parser: `KEY=VALUE` per line, supporting an optional `export`
/// prefix, `#` comments, blank lines, and single/double quoted values.
fn parse_dotenv_into(content: &str, map: &mut HashMap<String, String>) {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim();
            let value = strip_quotes(value);
            map.entry(key).or_insert_with(|| value.to_string());
        }
    }
}

fn strip_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// Resolves all `{{prefix NAME}}` occurrences in `text` by looking up `NAME`
/// via `lookup`. Unknown names resolve to an empty string. `prefix` is the
/// full directive head, e.g. `"{{$dotenv"`.
fn resolve_lookup<F>(text: &mut String, prefix: &str, lookup: F)
where
    F: Fn(&str) -> Option<String>,
{
    while let Some(start_idx) = text.find(prefix) {
        let Some(offset) = text[start_idx..].find("}}") else {
            break; // Malformed, prevent infinite loop
        };
        let full_match = text[start_idx..start_idx + offset + 2].to_string();
        let name = full_match
            .trim_start_matches("{{")
            .trim_end_matches("}}")
            .split_whitespace()
            .nth(1)
            .unwrap_or("");
        let value = lookup(name).unwrap_or_default();
        *text = text.replace(&full_match, &value);
    }
}

fn process_auth_header(value: &str) -> String {
    if let Some(stripped) = value.strip_prefix("Basic ") {
        let remainder = stripped.trim();

        // Accept plaintext credentials as either "username password" (space) or
        // "username:password" (colon) and base64-encode them. A single token
        // with neither separator is assumed to be already base64-encoded and is
        // left untouched (base64 never contains a space or colon).
        let credentials = if let Some((user, pass)) = remainder.split_once(' ') {
            Some(format!("{}:{}", user.trim(), pass.trim()))
        } else if let Some((user, pass)) = remainder.split_once(':') {
            Some(format!("{}:{}", user.trim(), pass.trim()))
        } else {
            None
        };

        if let Some(credentials) = credentials {
            let encoded = BASE64_STANDARD.encode(credentials);
            return format!("Basic {}", encoded);
        }
    }
    value.to_string()
}

fn resolve_system_variables(text: &str, dotenv: &HashMap<String, String>) -> String {
    let mut resolved = text.to_string();

    // {{$dotenv NAME}} — value from the nearest .env file
    resolve_lookup(&mut resolved, "{{$dotenv", |name| dotenv.get(name).cloned());

    // {{$processEnv NAME}} — value from the process environment
    resolve_lookup(&mut resolved, "{{$processEnv", |name| {
        std::env::var(name).ok()
    });

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

/// Removes whole-line comments from a request body. A line whose first
/// non-whitespace characters are `#` or `//` is dropped; all other lines are
/// kept verbatim. This lets users annotate request blocks (including trailing
/// comments before the next `###` separator) without those lines being sent.
fn strip_body_comments(body: &str) -> String {
    body.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with('#') || trimmed.starts_with("//"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn resolve_variables(
    text: &str,
    variables: &HashMap<&str, &str>,
    dotenv: &HashMap<String, String>,
) -> String {
    let mut resolved = text.to_string();

    // 1. Resolve custom user variables from the file
    for (key, value) in variables {
        let placeholder = format!("{{{{{}}}}}", key);
        resolved = resolved.replace(&placeholder, value);
    }

    // 2. Resolve built-in system variables (including $dotenv / $processEnv)
    resolve_system_variables(&resolved, dotenv)
}

/// Converts our parsed HttpRequest into a native reqwest::Request.
///
/// `base_dir`, when provided, is the directory of the request file and is used
/// to locate the nearest `.env` file for `{{$dotenv NAME}}` resolution.
pub fn build_request(
    client: &Client,
    req: &HttpRequest<'_>,
    variables: &HashMap<&str, &str>,
    base_dir: Option<&Path>,
) -> anyhow::Result<Request> {
    let method = Method::from_str(req.method)
        .map_err(|_| anyhow::anyhow!("Invalid HTTP Method: {}", req.method))?;

    let dotenv = base_dir.map(load_dotenv).unwrap_or_default();

    let url = resolve_variables(req.url, variables, &dotenv);
    let mut request_builder = client.request(method, &url);

    for (key, value) in &req.headers {
        let resolved_key = resolve_variables(key, variables, &dotenv);
        let mut resolved_value = resolve_variables(value, variables, &dotenv);

        if resolved_key.to_lowercase() == "authorization" {
            resolved_value = process_auth_header(&resolved_value);
        }

        request_builder = request_builder.header(resolved_key, resolved_value);
    }

    if let Some(body_text) = req.body {
        let stripped = strip_body_comments(body_text);
        let resolved_body = resolve_variables(&stripped, variables, &dotenv);
        // A body consisting only of comments/whitespace is treated as no body.
        if !resolved_body.trim().is_empty() {
            request_builder = request_builder.body(resolved_body);
        }
    }

    let request = request_builder
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build request: {}", e))?;

    Ok(request)
}

/// Renders a built request back into raw HTTP text (request line, headers, and
/// body) for display alongside the response. Header values — including
/// `Authorization` — are shown verbatim.
pub fn render_request(req: &Request) -> String {
    let mut out = format!("{} {}\n", req.method(), req.url());

    for (name, value) in req.headers() {
        let v = value.to_str().unwrap_or("[invalid header value]");
        out.push_str(&format!("{}: {}\n", name, v));
    }

    if let Some(body) = req.body()
        && let Some(bytes) = body.as_bytes()
        && !bytes.is_empty()
    {
        out.push('\n');
        out.push_str(&String::from_utf8_lossy(bytes));
        out.push('\n');
    }

    out
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

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), None)
            .expect("Failed to build request");

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
            build_request(&client, &http_req, &vars, None).expect("Failed to build request");

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
        let dotenv = HashMap::new();

        let guid_text = resolve_variables("id: {{$guid}}", &vars, &dotenv);
        assert!(guid_text.starts_with("id: "));
        assert_eq!(guid_text.len(), 4 + 36); // "id: " + 36 char UUID

        let dt_iso = resolve_variables("time: {{$datetime iso8601}}", &vars, &dotenv);
        assert!(dt_iso.contains('T')); // ISO8601 has a 'T'
        assert!(dt_iso.contains('+') || dt_iso.contains('Z'));

        let rand_int = resolve_variables("number: {{$randomInt 10 20}}", &vars, &dotenv);
        let num_str = rand_int.strip_prefix("number: ").unwrap();
        let num: i32 = num_str.parse().unwrap();
        assert!((10..=20).contains(&num));
    }

    #[test]
    fn test_parse_dotenv() {
        let content = "# a comment\n\
                       FOO=bar\n\
                       export BAZ = qux\n\
                       QUOTED=\"hello world\"\n\
                       SINGLE='single'\n\
                       EMPTY=\n";
        let mut map = HashMap::new();
        parse_dotenv_into(content, &mut map);
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(map.get("BAZ").map(String::as_str), Some("qux"));
        assert_eq!(map.get("QUOTED").map(String::as_str), Some("hello world"));
        assert_eq!(map.get("SINGLE").map(String::as_str), Some("single"));
        assert_eq!(map.get("EMPTY").map(String::as_str), Some(""));
        assert!(!map.contains_key("# a comment"));
    }

    #[test]
    fn test_dotenv_variable() {
        let vars = HashMap::new();
        let mut dotenv = HashMap::new();
        dotenv.insert("USERNAME".to_string(), "alice".to_string());

        // Direct usage in a request field.
        let resolved = resolve_variables("user: {{$dotenv USERNAME}}", &vars, &dotenv);
        assert_eq!(resolved, "user: alice");

        // Unknown name resolves to empty string.
        let missing = resolve_variables("x: {{$dotenv NOPE}}", &vars, &dotenv);
        assert_eq!(missing, "x: ");
    }

    #[test]
    fn test_dotenv_via_file_variable() {
        // Mirrors `@user = {{$dotenv USERNAME}}` followed by a request using {{user}}.
        let mut vars = HashMap::new();
        vars.insert("user", "{{$dotenv USERNAME}}");
        let mut dotenv = HashMap::new();
        dotenv.insert("USERNAME".to_string(), "bob".to_string());

        let resolved = resolve_variables("{{user}}@example.com", &vars, &dotenv);
        assert_eq!(resolved, "bob@example.com");
    }

    #[test]
    fn test_process_env_variable() {
        let vars = HashMap::new();
        let dotenv = HashMap::new();

        // SAFETY: single-threaded test; we set then read one variable.
        unsafe {
            std::env::set_var("SIDECAR_TEST_PROCENV", "from-proc-env");
        }
        let resolved = resolve_variables(
            "token: {{$processEnv SIDECAR_TEST_PROCENV}}",
            &vars,
            &dotenv,
        );
        assert_eq!(resolved, "token: from-proc-env");
    }

    #[test]
    fn test_build_request_with_dotenv_file() {
        use std::io::Write;

        // Create an isolated temp dir with a .env file.
        let dir = std::env::temp_dir().join(format!("sidecar_dotenv_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        let mut f = std::fs::File::create(&env_path).unwrap();
        writeln!(f, "TOKEN=secret-from-dotenv").unwrap();

        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "https://api.example.com/me",
            headers: vec![("Authorization", "Bearer {{$dotenv TOKEN}}")],
            body: None,
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), Some(dir.as_path()))
            .expect("Failed to build request");

        assert_eq!(
            reqwest_req.headers().get("Authorization").unwrap(),
            "Bearer secret-from-dotenv"
        );

        std::fs::remove_dir_all(&dir).ok();
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

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), None)
            .expect("Failed to build request");

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
            build_request(&client, &http_req, &vars, None).expect("Failed to build request");

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
    fn test_render_request() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "POST",
            url: "https://api.example.com/users",
            headers: vec![
                ("Content-Type", "application/json"),
                ("Authorization", "Bearer secret-token"),
            ],
            body: Some("{\"name\":\"John\"}"),
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), None)
            .expect("Failed to build request");

        let rendered = render_request(&reqwest_req);

        assert!(rendered.starts_with("POST https://api.example.com/users\n"));
        assert!(rendered.contains("content-type: application/json\n"));
        // Authorization is shown verbatim (cleartext).
        assert!(rendered.contains("authorization: Bearer secret-token\n"));
        assert!(rendered.ends_with("{\"name\":\"John\"}\n"));
    }

    #[test]
    fn test_render_request_no_body() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "https://api.example.com/ping",
            headers: vec![("Accept", "application/json")],
            body: None,
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), None)
            .expect("Failed to build request");

        let rendered = render_request(&reqwest_req);
        assert!(rendered.starts_with("GET https://api.example.com/ping\n"));
        assert!(rendered.contains("accept: application/json\n"));
        // No blank-line-separated body section when there is no body.
        assert!(!rendered.contains("\n\n"));
    }

    #[test]
    fn test_basic_auth_colon_form() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "https://httpbin.org/basic-auth/user/passwd",
            // Colon-separated credentials (the form many users write).
            headers: vec![("Authorization", "Basic user:passwd")],
            body: None,
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), None)
            .expect("Failed to build request");

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
    fn test_basic_auth_colon_form_with_variables() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "https://api.example.com/tokens",
            headers: vec![("Authorization", "Basic {{user}}:{{password}}")],
            body: None,
        };

        let mut vars = HashMap::new();
        vars.insert("user", "admin");
        vars.insert("password", "secret");

        let reqwest_req =
            build_request(&client, &http_req, &vars, None).expect("Failed to build request");

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
    fn test_basic_auth_preencoded_passthrough() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "https://api.example.com/tokens",
            // Already base64-encoded: must be left untouched.
            headers: vec![("Authorization", "Basic YWRtaW46c2VjcmV0")],
            body: None,
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), None)
            .expect("Failed to build request");

        let auth_header = reqwest_req
            .headers()
            .get("Authorization")
            .unwrap()
            .to_str()
            .unwrap();
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

    #[test]
    fn test_strip_body_comments() {
        let body = "{\n  \"a\": 1\n  # a comment\n  // another\n}\n# trailing";
        let stripped = strip_body_comments(body);
        assert_eq!(stripped, "{\n  \"a\": 1\n}");
    }

    #[test]
    fn test_build_request_strips_body_comments() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "POST",
            url: "https://api.example.com/values",
            headers: vec![("Content-Type", "application/json")],
            body: Some("# leading comment\n{\"a\":1}\n# trailing comment"),
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), None)
            .expect("Failed to build request");

        let body_bytes = reqwest_req.body().unwrap().as_bytes().unwrap();
        assert_eq!(body_bytes, b"{\"a\":1}");
    }

    #[test]
    fn test_build_request_comment_only_body_sends_no_body() {
        let client = Client::new();
        let http_req = HttpRequest {
            method: "GET",
            url: "https://api.example.com/values",
            headers: vec![("Authorization", "Bearer token")],
            // Matches the reported case: a stray comment captured as the body.
            body: Some("# some comment here"),
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), None)
            .expect("Failed to build request");

        assert!(reqwest_req.body().is_none());
    }
}
