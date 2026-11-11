[![Crates.io](https://shields.io)](https://crates.io)
[![Docs.rs](https://docs.rs)](https://docs.rs)
[![License: MIT](https://shields.io)](LICENSE)

# Kasi (`kasi-jb`)

**Kasi** is an async, macro-driven web and microservice framework for Rust built on **Hyper 1.x** and **Tokio**. It combines compile-time route and schema generation with sequential request extractors, RFC 7807 error responses, OpenID Connect JWT verification, static file serving, multipart uploads, and zero-boilerplate OpenAPI 3.1 documentation.

---

## Installation & CLI Scaffolder

### Scaffold a New Project with `kasi-cli`
Install the CLI scaffolder and generate a ready-to-run service in seconds:

```bash
cargo install kasi-cli
kasi new my-service
cd my-service
cargo run
```

### Add `kasi-jb` to an Existing Project

```bash
cargo add kasi-jb
```

Enable optional feature flags as needed in `Cargo.toml`:

```toml
[dependencies]
# Default enables both `postgres` (SeaORM + SQLx) and `templates` (MiniJinja)
kasi-jb = "0.5.1"

# Or opt into only what you need:
# kasi-jb = { version = "0.5.1", default-features = false, features = ["templates", "graphql"] }
```

---

## Quick Start

```rust
use kasi_jb::prelude::*;

#[endpoint(GET, "/hello/{name}")]
async fn greet(Path(name): Path<String>) -> String {
    format!("Hello, {}!", name)
}

#[main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 8080,
    })
    .middleware(TracingMiddleware::new())
    .route(greet)
    .build()
    .await?;

    // Listens on http://127.0.0.1:8080 and drains connections gracefully on Ctrl+C
    app.serve().await?;
    Ok(())
}
```

---

## Core Features

### 1. Declarative Routing & Regex Path Constraints
Use `#[endpoint(METHOD, "/path")]` to define async handlers and mount them directly with `.route(fn_name)`. Endpoints can return `Result<T, E>` or any `IntoResponse` type directly (`String`, `&'static str`, `JsonValue`, `Html`). Path parameters support inline regular expressions and deserialize into primitives or structs via `Path<T>`.

```rust
#[endpoint(GET, "/items/{sku:[A-Z]{3}-\\d+}")]
async fn get_item(Path(sku): Path<String>) -> JsonValue {
    json!({ "sku": sku, "in_stock": true })
}

#[endpoint(POST, "/graphql")]
async fn graphql_handler(
    Extension(schema): Extension<AppSchema>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

#[endpoint(GET, "/graphiql")]
async fn graphiql_ui() -> Response<BoxBody> {
    graphiql_playground("/graphql")
}
```

### 2. Sequential Request Extractors & Validation
Extractors consume and thread request ownership sequentially, allowing multiple extractors per handler without borrow or move conflicts:
* `Path<T>`: Extracts URL path parameters.
* `Query<T>`: Deserializes URL query strings.
* `Json<T>`: Deserializes JSON request bodies.
* `ValidatedJson<T>`: Deserializes JSON and runs `#[derive(Validate)]` rules automatically, returning an RFC 7807 `400 Bad Request` on failure.
* `Multipart`: Parses `multipart/form-data` text fields and binary file uploads (`UploadedFile`).
* `Cookies`: Extracts parsed `Cookie` headers into a key-value map.
* `Extension<T>`: Clones shared application state registered via `.extension(...)`.
* `RequestContext`: Provides the request URI, HTTP method, and active `trace_id`.

```rust
#[derive(Deserialize, Validate)]
struct CreateUser {
    #[validate(min_len = 3, regex = "^[a-z0-9_]+\$")]
    username: String,
    #[validate(email)]
    email: String,
    #[validate(range(min = 18, max = 120))]
    age: u8,
}

#[endpoint(POST, "/users")]
async fn create_user(
    ValidatedJson(user): ValidatedJson<CreateUser>,
) -> Result<JsonValue, Error> {
    Ok(json!({ "created": user.username }))
}
```

### 3. Multipart File Uploads
Extract text fields and uploaded files directly with `Multipart`:

```rust
#[endpoint(POST, "/upload")]
async fn upload_file(form: Multipart) -> JsonValue {
    let title = form.fields.get("title").cloned().unwrap_or_default();
    let count = form.files.len();
    json!({ "title": title, "uploaded_files": count })
}
```

### 4. Static File Serving & MiniJinja Templates
Serve static assets (CSS, JS, images) safely with path-traversal protection via `.serve_dir(url_prefix, dir)`, and render server-side HTML templates with `TemplateEngine` (`feature = "templates"`).

```rust
#[endpoint(GET, "/dashboard/{user}")]
async fn dashboard(
    engine: TemplateEngine,
    Path(user): Path<String>,
) -> Result<Html, Error> {
    engine.render("dashboard.html", template_context!(user => user))
}

// Mount static files and templates on ApplicationBuilder:
// app.extension(TemplateEngine::from_dir("templates"))
//    .serve_dir("/static", "public")
```

### 5. Cookie & Session Management
Read incoming browser cookies via `Cookies` and issue `HttpOnly` / `SameSite` cookies using `Cookie`:

```rust
#[endpoint(POST, "/session")]
async fn set_session() -> Response<BoxBody> {
    let mut res = "Session active".into_response();
    Cookie::new("sid", "session_token_123")
        .http_only(true)
        .same_site(SameSite::Strict)
        .max_age(3600)
        .append_to_headers(res.headers_mut());
    res
}

#[endpoint(GET, "/session")]
async fn read_session(cookies: Cookies) -> JsonValue {
    json!({ "sid": cookies.get("sid") })
}
```

### 6. Polymorphic Responses & RFC 7807 Problem Details
Derive `IntoResponse` on structs for `200 OK` JSON responses, and on enums for standardized `application/problem+json` error envelopes.

```rust
#[derive(Serialize, IntoResponse, Schema)]
struct UserDto {
    id: uuid::Uuid,
    username: String,
}

#[derive(IntoResponse)]
enum ApiError {
    #[problem(status = 404, title = "User Not Found")]
    NotFound,
    #[problem(status = 403, title = "Insufficient Permissions")]
    Forbidden,
}
```

### 7. Auto-Generated OpenAPI 3.1 Specs & Swagger UI
Annotate endpoints with `#[response(...)]` and derive `Schema` on your models. Kasi builds the OpenAPI 3.1.0 specification at startup and serves it automatically:
* **OpenAPI JSON:** `GET /openapi.json`
* **Interactive Swagger UI:** `GET /docs`

```rust
#[endpoint(GET, "/users/{id}")]
#[response(status = 200, description = "Fetched user record", json = UserDto)]
#[response(status = 404, description = "User does not exist")]
async fn fetch_user(Path(id): Path<uuid::Uuid>) -> Result<UserDto, ApiError> {
    Err(ApiError::NotFound)
}

// Register schemas on ApplicationBuilder:
// app.register_schema::<UserDto>().route(fetch_user)
```

### 8. OpenID Connect (OIDC) JWT Security
Protect routes declaratively with `#[security(...)]`. `OidcConfig::discover` fetches and caches your identity provider's JWKS (Keycloak, Auth0, Okta, Google) at startup for fast local signature verification.

```rust
#[endpoint(DELETE, "/admin/users/{id}")]
#[security(RequireRole = "Administrator", OAuth2 = ["users:delete"])]
async fn delete_user(
    auth: AuthContext,
    Path(id): Path<uuid::Uuid>,
) -> Result<&'static str, Error> {
    println!("Deleted {} by admin {}", id, auth.email);
    Ok("Deleted")
}
```

### 9. Database Pools & Automatic Transactions (`feature = "postgres"`)
Inject SeaORM (`Db`) or SQLx (`DbPool`) connections via `.extension(...)`, or use the `Tx` extractor to open a SeaORM transaction that commits automatically on `Ok` and rolls back on `Err`.

```rust
#[endpoint(POST, "/orders")]
async fn place_order(
    mut tx: Tx,
    Json(order): Json<NewOrder>,
) -> Result<&'static str, Error> {
    // Queries executed on `&tx.0` automatically commit on Ok or roll back on Err
    Ok("Order committed")
}
```

### 10. Modular Route Nesting & Built-In Middleware
Group endpoints under URL prefixes with isolated middleware using `Module`, and attach global middleware layers to `ApplicationBuilder`:
* `TracingMiddleware::new()` / `TracingMiddleware::json()`: Propagates `X-Request-Id` headers and logs request latency.
* `RateLimitMiddleware::new(max_requests, window)`: Fixed-window per-client throttling with `X-RateLimit-*` headers and `429` responses.
* `CorsMiddleware`: Configurable cross-origin resource sharing.

```rust
let inventory = Module::new("/inventory").route(get_item);

let api_v1 = Module::new("/api/v1")
    .wrap(from_fn(|req, next| async move { Ok(next.run(req).await) }))
    .nest_with_docs(inventory);

let app = Application::builder(Config { host: "127.0.0.1".into(), port: 8080 })
    .middleware(TracingMiddleware::json())
    .middleware(RateLimitMiddleware::new(100, std::time::Duration::from_secs(60)))
    .middleware(CorsMiddleware::permissive())
    .register_module(api_v1)
    .build()
    .await?;
```

### 11. Bidirectional WebSockets
Upgrade HTTP GET connections into full-duplex WebSockets with `WebSocketUpgrade`:

```rust
#[endpoint(GET, "/ws/echo")]
async fn ws_echo(ws: WebSocketUpgrade) -> Response<BoxBody> {
    ws.on_upgrade(|mut socket| async move {
        while let Some(Ok(WsMessage::Text(text))) = socket.recv().await {
            let _ = socket.send_text(format!("echo: {}", text)).await;
        }
        let _ = socket.close().await;
    })
}
```

### 12. Background Task Scheduling
Schedule recurring interval workers on `ApplicationBuilder` (automatically stopped during graceful shutdown) or extract `Extension<TaskScheduler>` in handlers to spawn background jobs:

```rust
let app = Application::builder(Config { host: "127.0.0.1".into(), port: 8080 })
    .schedule_task("cache-cleanup", std::time::Duration::from_secs(60), || async {
        println!("Running periodic maintenance...");
    })
    .route(ws_echo)
    .build()
    .await?;
```
### 13. Server-Sent Events (`Sse`)
Stream real-time server events over `text/event-stream` using `Sse` and `SseEvent`:

```rust
#[endpoint(GET, "/events/ticks")]
async fn live_ticks() -> Sse {
    let stream = futures_util::stream::unfold(1u64, |count| async move {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let event = SseEvent::data(format!("tick #{}", count))
            .event("heartbeat")
            .id(count.to_string());
        Some((Ok(event), count + 1))
    });

    Sse::new(stream)
}
```

### 14. Response Compression & Prometheus Observability
Enable automatic Gzip response compression and mount built-in `GET /healthz` and `GET /metrics` endpoints with a single builder call:

```rust
let app = Application::builder(Config { host: "127.0.0.1".into(), port: 8080 })
    .with_observability() // Mounts GET /healthz and GET /metrics
    .middleware(CompressionMiddleware::new())
    .route(live_ticks)
    .build()
    .await?;
```

---

## Workspace Crates

| Crate | Purpose |
| :--- | :--- |
| **`kasi-jb`** | Umbrella facade and `prelude` re-exporting the full framework API. |
| **`kasi-core`** | Hyper runtime, regex router, extractors, static files, cookies, middleware, templates, and OpenAPI generator. |
| **`kasi-macros`** | Procedural macros (`#[endpoint]`, `#[main]`, `#[derive(IntoResponse, Validate, Schema)]`). |
| **`kasi-security`** | OIDC discovery client, JWKS caching, and `AuthContext` RBAC/scope guards. |
| **`kasi-cli`** | Command-line project scaffolder (`kasi new <project-name>`). |

---

## License

MIT License — Copyright (c) 2026 Justice Sam.
