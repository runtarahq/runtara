# runtara-agents

[![Crates.io](https://img.shields.io/crates/v/runtara-agents.svg)](https://crates.io/crates/runtara-agents)
[![Documentation](https://docs.rs/runtara-agents/badge.svg)](https://docs.rs/runtara-agents)
[![License](https://img.shields.io/crates/l/runtara-agents.svg)](LICENSE)

Built-in agent implementations for [Runtara](https://runtara.com) workflows. Provides ready-to-use integrations for HTTP, SFTP, CSV, XML, and data transformation.

## Overview

Agents are reusable components that execute specific operations within workflows. This crate provides:

- **HTTP Agent**: Make HTTP requests with authentication support
- **SFTP Agent**: Upload and download files via SFTP
- **CSV Agent**: Parse and generate CSV data
- **XML Agent**: Parse XML documents and extract data
- **Transform Agent**: Map, filter, and transform data
- **Text Agent**: String manipulation and formatting
- **Utils Agent**: Utilities like random generation, hashing, encoding

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
runtara-agents = "1.0"
```

## Built-in Agents

### HTTP Agent

Make HTTP requests with various authentication methods:

```rust
use runtara_agents::http;

// Simple GET request
let response = http::request(http::RequestInput {
    url: "https://api.example.com/data".to_string(),
    method: Some("GET".to_string()),
    headers: None,
    body: None,
    timeout_seconds: Some(30),
}).await?;

println!("Status: {}", response.status_code);
println!("Body: {}", response.body);
```

**Capabilities:**
- `request`: Generic HTTP request
- Supports Bearer tokens, API keys, and basic auth via connection extractors

### SFTP Agent

Transfer files via SFTP:

```rust
use runtara_agents::sftp;

// Upload a file
sftp::upload(sftp::UploadInput {
    host: "sftp.example.com".to_string(),
    port: Some(22),
    username: "user".to_string(),
    password: Some("pass".to_string()),
    private_key: None,
    remote_path: "/uploads/file.txt".to_string(),
    content: "File contents here".to_string(),
}).await?;

// Download a file
let result = sftp::download(sftp::DownloadInput {
    host: "sftp.example.com".to_string(),
    port: Some(22),
    username: "user".to_string(),
    password: Some("pass".to_string()),
    private_key: None,
    remote_path: "/downloads/file.txt".to_string(),
}).await?;

println!("Downloaded: {}", result.content);
```

**Capabilities:**
- `upload`: Upload file content to remote path
- `download`: Download file from remote path
- `list`: List directory contents

### CSV Agent

Parse and generate CSV data:

```rust
use runtara_agents::csv;

// Parse CSV string
let result = csv::parse(csv::ParseInput {
    content: "name,age\nAlice,30\nBob,25".to_string(),
    delimiter: Some(",".to_string()),
    has_headers: Some(true),
}).await?;

for row in result.rows {
    println!("{:?}", row);
}

// Generate CSV from records
let output = csv::generate(csv::GenerateInput {
    records: vec![
        serde_json::json!({"name": "Alice", "age": 30}),
        serde_json::json!({"name": "Bob", "age": 25}),
    ],
    columns: vec!["name".to_string(), "age".to_string()],
    delimiter: Some(",".to_string()),
    include_headers: Some(true),
}).await?;

println!("{}", output.content);
```

**Capabilities:**
- `parse`: Parse CSV string to structured data
- `generate`: Generate CSV from records

### XML Agent

Parse XML documents:

```rust
use runtara_agents::xml;

let result = xml::parse(xml::ParseInput {
    content: "<root><item>value</item></root>".to_string(),
}).await?;

// Access parsed structure
println!("{:?}", result.data);
```

**Capabilities:**
- `parse`: Parse XML to JSON structure
- `extract`: Extract values using XPath-like expressions

### Transform Agent

Transform and map data:

```rust
use runtara_agents::transform;

// Map fields between structures
let result = transform::map_fields(transform::MapFieldsInput {
    source: serde_json::json!({"firstName": "John", "lastName": "Doe"}),
    mapping: serde_json::json!({
        "full_name": "{{ firstName }} {{ lastName }}",
        "first": "{{ firstName }}"
    }),
}).await?;

// Filter array
let filtered = transform::filter(transform::FilterInput {
    items: vec![
        serde_json::json!({"age": 25}),
        serde_json::json!({"age": 35}),
        serde_json::json!({"age": 20}),
    ],
    condition: "age >= 25".to_string(),
}).await?;
```

**Capabilities:**
- `map-fields`: Map and transform fields using templates
- `filter`: Filter arrays based on conditions
- `merge`: Merge multiple objects

### Text Agent

String manipulation:

```rust
use runtara_agents::text;

// Format string with template
let result = text::format(text::FormatInput {
    template: "Hello, {{ name }}!".to_string(),
    values: serde_json::json!({"name": "World"}),
}).await?;

// Split string
let parts = text::split(text::SplitInput {
    text: "a,b,c".to_string(),
    delimiter: ",".to_string(),
}).await?;
```

**Capabilities:**
- `format`: Format strings with templates (Jinja2-like)
- `split`: Split string into array
- `join`: Join array into string
- `replace`: Find and replace

### Utils Agent

Utility operations:

```rust
use runtara_agents::utils;

// Generate random string
let random = utils::random_string(utils::RandomStringInput {
    length: 16,
    charset: Some("alphanumeric".to_string()),
}).await?;

// Hash data
let hash = utils::hash(utils::HashInput {
    data: "secret".to_string(),
    algorithm: "sha256".to_string(),
}).await?;

// Base64 encode/decode
let encoded = utils::base64_encode(utils::Base64EncodeInput {
    data: "Hello".to_string(),
}).await?;
```

**Capabilities:**
- `random-string`: Generate random strings
- `hash`: SHA-256 hashing
- `base64-encode`/`base64-decode`: Base64 encoding

## Connection Extractors

Agents support automatic credential extraction from connection configurations:

```rust
use runtara_agents::extractors::{HttpBearerExtractor, HttpApiKeyExtractor, SftpExtractor};

// Extract Bearer token for HTTP requests
let extractor = HttpBearerExtractor::new();
let headers = extractor.extract(&connection_config)?;

// Extract API key
let extractor = HttpApiKeyExtractor::new();
let headers = extractor.extract(&connection_config)?;

// Extract SFTP credentials
let extractor = SftpExtractor::new();
let credentials = extractor.extract(&connection_config)?;
```

## Agent Registry

Access all registered agents programmatically:

```rust
use runtara_agents::registry::get_all_agents;

let agents = get_all_agents();
for agent in agents {
    println!("Agent: {} - {}", agent.id, agent.description);
    for cap in &agent.capabilities {
        println!("  Capability: {} - {}", cap.id, cap.description);
    }
}
```

## Related Crates

- [`runtara-dsl`](https://crates.io/crates/runtara-dsl) - DSL type definitions
- [`runtara-agent-macro`](https://crates.io/crates/runtara-agent-macro) - Define custom agents
- [`runtara-workflow-stdlib`](https://crates.io/crates/runtara-workflow-stdlib) - Standard library including these agents

## License

This project is licensed under [AGPL-3.0-or-later](LICENSE).
