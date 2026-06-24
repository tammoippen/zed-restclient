use std::collections::HashMap;

#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub struct HttpRequest<'a> {
    pub name: Option<&'a str>,
    pub method: &'a str,
    pub url: &'a str,
    pub headers: Vec<(&'a str, &'a str)>,
    pub body: Option<&'a str>,
}

/// Parse a `# @name <name>` / `// @name <name>` / `# @name=<name>` marker line.
/// Returns the captured name when the trimmed line is such a marker.
fn parse_name_marker(trimmed: &str) -> Option<&str> {
    let rest = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("//"))?
        .trim_start();
    let rest = rest.strip_prefix("@name")?;
    // Must be followed by whitespace or '='; otherwise it's e.g. "@namespace".
    let rest = match rest.chars().next() {
        Some('=') => &rest[1..],
        Some(c) if c.is_whitespace() => rest,
        None => return None,
        _ => return None,
    };
    let name = rest.trim();
    if name.is_empty() { None } else { Some(name) }
}

#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub struct HttpFile<'a> {
    pub requests: Vec<HttpRequest<'a>>,
    pub variables: HashMap<&'a str, &'a str>,
}

#[allow(dead_code)]
pub fn parse_http_file(content: &str) -> HttpFile<'_> {
    let mut requests = Vec::new();
    let mut variables = HashMap::new();
    let mut looking_for_request = true;

    let mut current_method = "GET";
    let mut current_url = "";
    let mut current_headers = Vec::new();
    let mut current_name: Option<&str> = None;
    let mut parsing_body = false;
    let mut body_start_idx = None;
    let mut body_end_idx = None;

    let content_ptr = content.as_ptr() as usize;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("###") {
            // Separator found. Finish the current request if we were building one.
            if !looking_for_request && !current_url.is_empty() {
                let body = if let (Some(s), Some(e)) = (body_start_idx, body_end_idx) {
                    if s <= e && s < content.len() {
                        let b = &content[s..e];
                        if b.trim().is_empty() {
                            None
                        } else {
                            Some(b.trim_end())
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                requests.push(HttpRequest {
                    name: current_name,
                    method: current_method,
                    url: current_url,
                    headers: current_headers.clone(),
                    body,
                });
            }

            // Reset state for the next request
            looking_for_request = true;
            current_method = "GET";
            current_url = "";
            current_headers.clear();
            current_name = None;
            parsing_body = false;
            body_start_idx = None;
            body_end_idx = None;
            continue;
        }

        if looking_for_request {
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
                // A comment line may carry a `@name` marker for the next request.
                if let Some(name) = parse_name_marker(trimmed) {
                    current_name = Some(name);
                }
                continue;
            }

            // Check for file variables
            if trimmed.starts_with('@')
                && let Some((name, value)) = trimmed.split_once('=')
            {
                // Extract variable name without the '@'
                variables.insert(name[1..].trim(), value.trim());
                continue;
            }

            // We found the request line!
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if !parts.is_empty() {
                if parts.len() >= 2 {
                    current_method = parts[0];
                    current_url = parts[1];
                } else {
                    current_method = "GET";
                    current_url = parts[0];
                }
                looking_for_request = false;
            }
            continue;
        }

        // If we are not looking for a request, we are parsing headers or body
        let line_end_ptr = line.as_ptr() as usize - content_ptr + line.len();

        if parsing_body {
            // Just update the end index of the body
            body_end_idx = Some(line_end_ptr);
        } else {
            if trimmed.is_empty() {
                // Empty line separates headers from body
                parsing_body = true;
                // The body starts after this empty line
                let start_ptr = line.as_ptr() as usize - content_ptr + line.len();
                // Check if there is a newline character to skip
                let actual_start = if start_ptr < content.len()
                    && content[start_ptr..].starts_with('\n')
                {
                    start_ptr + 1
                } else if start_ptr + 1 < content.len() && content[start_ptr..].starts_with("\r\n")
                {
                    start_ptr + 2
                } else {
                    start_ptr
                };

                body_start_idx = Some(actual_start);
                body_end_idx = Some(actual_start);
            } else if trimmed.starts_with('#') || trimmed.starts_with("//") {
                // Ignore comments in headers
                continue;
            } else if let Some((key, value)) = trimmed.split_once(':') {
                current_headers.push((key.trim(), value.trim()));
            }
        }
    }

    // Push the final request if there is one
    if !looking_for_request && !current_url.is_empty() {
        let body = if let (Some(s), Some(e)) = (body_start_idx, body_end_idx) {
            if s <= e && s < content.len() {
                let b = &content[s..e];
                if b.trim().is_empty() {
                    None
                } else {
                    Some(b.trim_end())
                }
            } else {
                None
            }
        } else {
            None
        };

        requests.push(HttpRequest {
            name: current_name,
            method: current_method,
            url: current_url,
            headers: current_headers,
            body,
        });
    }

    HttpFile {
        requests,
        variables,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_get() {
        let content = "GET https://api.example.com/users";
        let file = parse_http_file(content);
        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].method, "GET");
        assert_eq!(file.requests[0].url, "https://api.example.com/users");
        assert!(file.requests[0].headers.is_empty());
        assert_eq!(file.requests[0].body, None);
        assert!(file.variables.is_empty());
    }

    #[test]
    fn test_parse_post_with_headers_and_body() {
        let content = r#"
POST https://api.example.com/users
Content-Type: application/json
Authorization: Bearer token123

{
    "name": "John Doe",
    "email": "john@example.com"
}
"#;
        let file = parse_http_file(content);
        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].method, "POST");
        assert_eq!(file.requests[0].url, "https://api.example.com/users");
        assert_eq!(file.requests[0].headers.len(), 2);
        assert_eq!(
            file.requests[0].headers[0],
            ("Content-Type", "application/json")
        );
        assert_eq!(
            file.requests[0].headers[1],
            ("Authorization", "Bearer token123")
        );
        assert_eq!(
            file.requests[0].body,
            Some("{\n    \"name\": \"John Doe\",\n    \"email\": \"john@example.com\"\n}")
        );
    }

    #[test]
    fn test_parse_multiple_requests() {
        let content = r#"
GET http://localhost:8080/1
###
PUT http://localhost:8080/2
"#;
        let file = parse_http_file(content);
        assert_eq!(file.requests.len(), 2);
        assert_eq!(file.requests[0].method, "GET");
        assert_eq!(file.requests[1].method, "PUT");
    }

    #[test]
    fn test_parse_name_hash() {
        let content = "# @name login\nPOST https://example.com/login\n";
        let file = parse_http_file(content);
        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].name, Some("login"));
    }

    #[test]
    fn test_parse_name_slash() {
        let content = "// @name login\nPOST https://example.com/login\n";
        let file = parse_http_file(content);
        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.requests[0].name, Some("login"));
    }

    #[test]
    fn test_parse_name_equals() {
        let content = "# @name=login\nPOST https://example.com/login\n";
        let file = parse_http_file(content);
        assert_eq!(file.requests[0].name, Some("login"));
    }

    #[test]
    fn test_name_resets_across_separator() {
        let content = "# @name login\nGET http://a/1\n###\nGET http://a/2\n";
        let file = parse_http_file(content);
        assert_eq!(file.requests.len(), 2);
        assert_eq!(file.requests[0].name, Some("login"));
        assert_eq!(file.requests[1].name, None);
    }

    #[test]
    fn test_no_name_is_none() {
        let content = "GET https://api.example.com/users";
        let file = parse_http_file(content);
        assert_eq!(file.requests[0].name, None);
    }

    #[test]
    fn test_parse_variables() {
        let content = r#"
@baseUrl = https://api.example.com
@token = supersecret

GET {{baseUrl}}/users
Authorization: Bearer {{token}}
"#;
        let file = parse_http_file(content);
        assert_eq!(file.requests.len(), 1);
        assert_eq!(file.variables.len(), 2);
        assert_eq!(
            file.variables.get("baseUrl"),
            Some(&"https://api.example.com")
        );
        assert_eq!(file.variables.get("token"), Some(&"supersecret"));
        assert_eq!(file.requests[0].url, "{{baseUrl}}/users");
    }
}
