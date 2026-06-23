use crate::exchange::ExchangeCache;
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

fn resolve_variables(
    text: &str,
    variables: &HashMap<&str, &str>,
    request_cache: &ExchangeCache,
) -> String {
    let mut resolved = text.to_string();

    // 1. Resolve custom user variables from the file (their values may
    //    themselves contain request-variable references).
    for (key, value) in variables {
        let placeholder = format!("{{{{{}}}}}", key);
        resolved = resolved.replace(&placeholder, value);
    }

    // 2. Resolve request variables ({{name.response.body.$...}}) from the cache.
    resolved = crate::exchange::resolve_request_variables(&resolved, request_cache);

    // 3. Resolve built-in system variables
    resolve_system_variables(&resolved)
}

/// Converts our parsed HttpRequest into a native reqwest::Request.
pub fn build_request(
    client: &Client,
    req: &HttpRequest<'_>,
    variables: &HashMap<&str, &str>,
    request_cache: &ExchangeCache,
) -> anyhow::Result<Request> {
    let method = Method::from_str(req.method)
        .map_err(|_| anyhow::anyhow!("Invalid HTTP Method: {}", req.method))?;

    let url = resolve_variables(req.url, variables, request_cache);
    let mut request_builder = client.request(method, &url);

    for (key, value) in &req.headers {
        let resolved_key = resolve_variables(key, variables, request_cache);
        let mut resolved_value = resolve_variables(value, variables, request_cache);

        if resolved_key.to_lowercase() == "authorization" {
            resolved_value = process_auth_header(&resolved_value);
        }

        request_builder = request_builder.header(resolved_key, resolved_value);
    }

    if let Some(body_text) = req.body {
        let resolved_body = resolve_variables(body_text, variables, request_cache);
        request_builder = request_builder.body(resolved_body);
    }

    let request = request_builder
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build request: {}", e))?;

    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Method;

    #[test]
    fn test_build_request() {
        let client = Client::new();
        let http_req = HttpRequest {
            name: None,
            method: "POST",
            url: "https://httpbin.org/post",
            headers: vec![("Content-Type", "application/json"), ("X-Custom", "Test")],
            body: Some("{\"hello\":\"world\"}"),
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), &ExchangeCache::new())
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
            name: None,
            method: "GET",
            url: "{{baseUrl}}/api/{{userId}}",
            headers: vec![("Authorization", "Bearer {{token}}")],
            body: Some("{\"id\":\"{{userId}}\"}"),
        };

        let mut vars = HashMap::new();
        vars.insert("baseUrl", "https://api.example.com");
        vars.insert("userId", "123");
        vars.insert("token", "secret123");

        let reqwest_req = build_request(&client, &http_req, &vars, &ExchangeCache::new())
            .expect("Failed to build request");

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
    fn test_build_request_with_request_variables() {
        use crate::exchange::{Exchange, ExchangeMessage};

        let client = Client::new();
        let http_req = HttpRequest {
            name: None,
            method: "GET",
            url: "https://api.example.com/comments/{{login.response.body.$.id}}",
            headers: vec![(
                "Authorization",
                "Bearer {{login.response.headers.X-AuthToken}}",
            )],
            body: None,
        };

        let mut cache = ExchangeCache::new();
        cache.insert(
            "login".to_string(),
            Exchange {
                request: ExchangeMessage::default(),
                response: ExchangeMessage {
                    headers: vec![("X-AuthToken".to_string(), "tok-99".to_string())],
                    body: r#"{"id":7}"#.to_string(),
                },
            },
        );

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), &cache)
            .expect("Failed to build request");

        assert_eq!(
            reqwest_req.url().as_str(),
            "https://api.example.com/comments/7"
        );
        assert_eq!(
            reqwest_req.headers().get("Authorization").unwrap(),
            "Bearer tok-99"
        );
    }

    #[test]
    fn test_system_variables() {
        let vars = HashMap::new();

        let guid_text = resolve_variables("id: {{$guid}}", &vars, &ExchangeCache::new());
        assert!(guid_text.starts_with("id: "));
        assert_eq!(guid_text.len(), 4 + 36); // "id: " + 36 char UUID

        let dt_iso = resolve_variables("time: {{$datetime iso8601}}", &vars, &ExchangeCache::new());
        assert!(dt_iso.contains('T')); // ISO8601 has a 'T'
        assert!(dt_iso.contains('+') || dt_iso.contains('Z'));

        let rand_int =
            resolve_variables("number: {{$randomInt 10 20}}", &vars, &ExchangeCache::new());
        let num_str = rand_int.strip_prefix("number: ").unwrap();
        let num: i32 = num_str.parse().unwrap();
        assert!((10..=20).contains(&num));
    }

    #[test]
    fn test_basic_auth_encoding() {
        let client = Client::new();
        let http_req = HttpRequest {
            name: None,
            method: "GET",
            url: "https://httpbin.org/basic-auth/user/passwd",
            headers: vec![("Authorization", "Basic user passwd")],
            body: None,
        };

        let reqwest_req = build_request(&client, &http_req, &HashMap::new(), &ExchangeCache::new())
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
            name: None,
            method: "GET",
            url: "https://httpbin.org/basic-auth/admin/secret",
            headers: vec![("Authorization", "Basic {{user}} {{pass}}")],
            body: None,
        };

        let mut vars = HashMap::new();
        vars.insert("user", "admin");
        vars.insert("pass", "secret");

        let reqwest_req = build_request(&client, &http_req, &vars, &ExchangeCache::new())
            .expect("Failed to build request");

        let auth_header = reqwest_req
            .headers()
            .get("Authorization")
            .unwrap()
            .to_str()
            .unwrap();
        // "admin:secret" in base64 is "YWRtaW46c2VjcmV0"
        assert_eq!(auth_header, "Basic YWRtaW46c2VjcmV0");
    }
}
