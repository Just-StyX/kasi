# Kasi (`kasi-jb`)

**Kasi** is an async, macro-driven web and microservice framework for Rust built on **Hyper 1.x** and **Tokio**. It combines compile-time route and schema generation with sequential request extractors, RFC 7807 error responses, OpenID Connect JWT verification, and zero-boilerplate OpenAPI 3.1 documentation.

---

## Installation

Add `kasi-jb` to your project:

```bash
cargo add kasi-jb
```

Enable optional feature flags as needed:

```toml
[dependencies]
# Default enables both `postgres` (SeaORM + SQLx) and `templates` (MiniJinja)
kasi-jb = "0.1.11"

# Or opt into only what you need:
# kasi-jb = { version = "0.1.11", default-features = false, features = ["templates"] }
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
Use `#[endpoint(METHOD, "/path")]` to define async handlers and mount them directly with `.route(fn_name)`. Path parameters support inline regular expressions and deserialize into primitives or structs via `Path<T>`.

```rust
#[endpoint(GET, "/items/{sku:[A-Z]{3}-\\d+}")]
async fn get_item(Path(sku): Path<String>) -> JsonValue {
    json!({ "sku": sku, "in_stock": true })
}
```

### 2. Sequential Request Extractors & Validation
Extractors consume and thread request ownership sequentially, allowing multiple extractors per handler without borrow or move conflicts:
* `Path<T>`: Extracts URL path parameters.
* `Query<T>`: Deserializes URL query strings.
* `Json<T>`: Deserializes JSON request bodies.
* `ValidatedJson<T>`: Deserializes JSON and runs `#[derive(Validate)]` rules automatically, returning an RFC 7807 `400 Bad Request` on failure.
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

### 3. Polymorphic Responses & RFC 7807 Problem Details
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

### 4. Auto-Generated OpenAPI 3.1 Specs & Swagger UI
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

### 5. OpenID Connect (OIDC) JWT Security
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

### 6. Database Pools & Automatic Transactions (`feature = "postgres"`)
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

### 7. Server-Side HTML Templates (`feature = "templates"`)
Render MiniJinja templates from a directory or embedded environment using `TemplateEngine` and `Html`.

```rust
#[endpoint(GET, "/dashboard/{user}")]
async fn dashboard(
    engine: TemplateEngine,
    Path(user): Path<String>,
) -> Result<Html, Error> {
    engine.render("dashboard.html", template_context!(user => user))
}
```

### 8. Modular Route Nesting & Built-In Middleware
Group endpoints under URL prefixes with isolated middleware using `Module`, and attach global middleware layers to `ApplicationBuilder`:
* `TracingMiddleware::new()` / `TracingMiddleware::json()`: Propagates `X-Request-Id` headers and logs request latency.
* `RateLimitMiddleware::new(max_requests, window)`: Fixed-window per-client throttling with `X-RateLimit-*` headers and `429` responses.
* `CorsMiddleware`: Configurable cross-origin resource sharing.

```rust
let inventory = Module::new("/inventory").route(get_item);

let api_v1 = Module::new("/api/v1")
    .wrap(from_fn(|req, next| async move { next.run(req).await }))
    .nest_with_docs(inventory);

let app = Application::builder(Config { host: "127.0.0.1".into(), port: 8080 })
    .middleware(TracingMiddleware::json())
    .middleware(RateLimitMiddleware::new(100, std::time::Duration::from_secs(60)))
    .middleware(CorsMiddleware::permissive())
    .register_module(api_v1)
    .build()
    .await?;
```

---

## Workspace Crates

| Crate | Purpose |
| :--- | :--- |
| **`kasi-jb`** | Umbrella facade and `prelude` re-exporting the full framework API. |
| **`kasi-core`** | Hyper runtime, regex router, extractors, middleware, templates, and OpenAPI generator. |
| **`kasi-macros`** | Procedural macros (`#[endpoint]`, `#[main]`, `#[derive(IntoResponse, Validate, Schema)]`). |
| **`kasi-security`** | OIDC discovery client, JWKS caching, and `AuthContext` RBAC/scope guards. |

---

## License

MIT License — Copyright (c) 2026 Justice Sam.
