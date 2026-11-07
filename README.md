# Kasi Framework

Kasi is an enterprise-grade, compile-time optimized microservice framework for modern Rust web development. Built cleanly on top of Hyper 1.x, Tokio, and `http_body_util`, Kasi provides developers with powerful multi-crate structural primitives, macro-driven declarative validation batteries, a zero-cost Type Map dependency injection model, and high-performance in-memory testing suites.

---

## ⚡ Architecture & Milestones Summary

We have engineered and stabilized a highly modular web engine designed for low-latency, thread-safe microservice patterns. By shifting constraints from runtime checks to compile-time analysis and strict type design, Kasi completely eliminates typical runtime type mismatches and closure lifetime escape bugs.

### 🌟 Core Framework Pillars

* **Unified Stream Domain:** Decoupled the microservice architecture from Hyper's private, sealed stream types by standardizing the request-response lifecycle entirely on a type-erased `Request<BoxBody>` abstraction layer.
* **Sequential Ownership-Threading Extractors:** Developed a by-value ownership-threading pattern (`Request<BoxBody> -> (Self, Request<BoxBody>)`) for parameter extraction. This allows endpoint handlers to extract multiple inputs (e.g., an auth profile, a database connection, and a payload validation check) sequentially without triggering "use of moved value" compiler crashes.
* **Zero-Allocation Global Middleware:** Refactored the `Middleware` execution pipeline to utilize explicit `'static` future returns. This allows the Tokio runtime thread pool to schedule work without generic lifetime reference friction.
* **Hierarchical Route Nesting:** Enabled clean grouping and prefixing via `Module::nest_with_docs`, ensuring parent middleware layers cascade into nested scopes without cross-contamination.
* **Advanced Multi-Strategy Regex Matcher:** Engineered a routing system that compiles parametric path variables and explicit constraints (e.g., `{id:\\d+}`) into organized, compiled regular expressions at server boot, providing a fast route resolution gate.
* **Intelligent JSON Path Extractor:** Upgraded the `Path<T>` parsing mechanics to build in-memory JSON Value nodes dynamically, allowing special characters like hyphens (`-`) or slashes to bypass strict URL-encoded form constraints, supporting both custom struct parameters and raw primitives (`u64`, `String`) simultaneously.

---

## Architecture Overview

Kasi is organized as a decoupled, multi-crate Cargo Workspace to ensure high cohesion, low compilation coupling, and clean division of architectural boundaries.

* **`kasi` (Umbrella Facade):** The main library entry point exposing a unified framework interface via its `prelude` module for maximum framework Developer Experience (DX).
* **`kasi-core` (Execution Engine):** Orchestrates the native HTTP socket lifecycle, routing, extractor implementations, global type-map application states, and the mock network testing subsystem.
* **`kasi-macros` (Meta-Compilation Suite):** Powers the system's compile-time optimizations, converting clean declarative macros into runtime execution branches and OpenAPI schemas.
* **`kasi-security` (OIDC Guard Core):** Provides local signature verification of incoming JSON Web Tokens (JWT) using cached JSON Web Key Sets (JWKS) discoverable via standard OpenID Connect specifications.

---

## Central Workspace Configuration (`Cargo.toml`)

This root manifest coordinates identical dependency variations and paths across the independent sub-crates to maintain lean compilation graphs.

```toml
[workspace]
resolver = "2"
members = [
    ".",
    "kasi-core",
    "kasi-macros",
    "kasi-security",
]

[workspace.package]
version = "0.1.10"
edition = "2024"
authors = ["Justice Sam <fjaccobian@gmail.com>"]
license = "MIT"
repository = "https://github.com"

[workspace.dependencies]
kasi-core = { path = "./kasi-core" }
kasi-macros = { path = "./kasi-macros" }
kasi-security = { path = "./kasi-security" }

tokio = { version = "1.54.1", features = ["full"] }
hyper = { version = "1.12.0", features = ["full"] }
hyper-util = { version = "0.1.21", features = ["server-auto", "tokio"] }
tower = { version = "0.5.3", features = ["util"] }
http = "1.5.0"
http-body-util = { version = "0.1.2", features = ["full"] }
bytes = "1.6.0"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.154"
syn = { version = "3.0.9", features = ["full", "extra-traits"] }
quote = "1.0.47"
proc-macro2 = "1.0.107"
proc-macro-error = "1.0.4"
serde_urlencoded = "0.7.1"
uuid = { version = "1.29.0", features = ["v4", "serde"]}
jsonwebtoken = "9.3.0"
openidconnect = "4.0.0"

sqlx = { version = "0.9.0", features = ["runtime-tokio", "tls-native-tls", "postgres", "uuid", "chrono"] }
sea-orm = { version = "2.0.4", features = ["sqlx-postgres", "runtime-tokio-native-tls", "macros"] }
```

---

## Core Usage Guide

### 1. Application Entry Point (`#[main]`)
The `#[main]` attribute abstracts the explicit allocation loop of multi-threaded Tokio scheduling engines while allowing standard synchronous error propagation types out of the application root.

```rust
use kasi::prelude::*;

#[main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config { host: "127.0.0.1".into(), port: 8080 };
    
    let app = Application::builder(config)
        .route(get_profile)
        .build()
        .await?;

    app.serve().await?;
    Ok(())
}
```

### 2. Declarative Route Metadata (`#[endpoint]`)
The `#[endpoint]` macro processes input arguments, hooks dynamic parameter extractors, and registers metadata mapping. Endpoints are registered by passing the actual function identity pointer into the router matrix cleanly.

```rust
#[endpoint(GET, "/v1/profile")]
async fn get_profile() -> &'static str {
    "Welcome to Kasi!"
}
```

### 3. Strongly-Typed Extraction Subsystem
Kasi includes robust extractors that consume request bodies, queries, and path maps by value exactly once, or pull state information out of context.

```rust
#[derive(Deserialize)]
struct UpdateProfile {
    nickname: String,
}

#[derive(Deserialize)]
struct FilterQuery {
    verbose: bool,
}

#[endpoint(POST, "/v1/profile")]
async fn update_profile(
    body: Json<UpdateProfile>,
    query: Query<FilterQuery>,
) -> &'static str {
    if query.0.verbose {
        println!("Updating profile target string to: {}", body.0.nickname);
    }
    "Profile Changed Successfully"
}
```

---

## Advanced Batteries

### 1. Unified General OpenID Connect Authentication Guard
Kasi natively maps generic OIDC authentication contexts (such as Keycloak, Auth0, or Google) by fetching target JSON Web Key Sets (JWKS) via standard discovery endpoints at startup, running high-fidelity local cryptographic validation against incoming HTTP bearer headers.

The macro uses the extracted data by reference (`&req`) to enforce roles and scopes *before* consuming the HTTP entity body, allowing the resulting `AuthContext` to be cloned directly into your route input signatures without triggering dual-extraction runtime failures.

```rust
#[endpoint(POST, "/v1/admin/purge")]
#[security(RequireRole = "Administrator", OAuth2 = ["system:write"])]
async fn administrative_purge(
    auth: AuthContext, // Clones and injects the context safely from the validation layer
    body: Json<PurgePayload>
) -> &'static str {
    println!("Purge command authorized by identity context: {}", auth.email);
    "Purge sequence dispatched."
}
```

### 2. Zero-Cost Polymorphic Responses (`#[derive(IntoResponse)]`)
The custom `IntoResponse` derive macro evaluates the semantic structure of target items at compile time to apply deterministic encoding configurations:
* **Struct Definitions:** Automatically compiles into conventional `200 OK` JSON payloads setting the matching `application/json` header boundary.
* **Enum Definitions:** Automatically maps parameters to render uniform **RFC 7807 Problem Details** layouts utilizing the `application/problem+json` standard specification.

```rust
// Compiles into a 200 OK structural application/json stream
#[derive(Serialize, IntoResponse)]
struct ProfileReport {
    username: String,
    active: bool,
}

// Compiles into a spec-compliant application/problem+json response envelope
#[derive(IntoResponse)]
enum DomainError {
    #[problem(status = 404, title = "Account Record Corrupted or Missing")]
    AccountNotFound,
    #[problem(status = 400, title = "Invalid Transaction Schema Inputs")]
    ValidationFailure,
}
```

### 3. Isolated Lock-Free Type Map Extensions
Kasi utilizes an atomic `http::Extensions` cache wrapper inside its connection pool to support clean dependency injection. State elements like database engines or credentials caches are registered during configuration loops and become shareable across thread pools seamlessly.

```rust
// Database connections are extracted cleanly using type-safe extension parameters
impl FromRequest for Db {
    fn extract(req: Request<Incoming>) -> BoxFuture<'static, Result<Self, Error>> {
        Box::pin(async move {
            let conn = req.extensions().get::<sea_orm::DatabaseConnection>().cloned()
                .ok_or_else(|| Error::Internal("SeaORM engine not initialized.".into()))?;
            Ok(Db(conn))
        })
    }
}
```

### 4. Scoped Atomic Database Transaction Capsules
To avoid lifetime and future variable ownership traps across dynamic macro evaluations, Kasi implements a scoped, closure-based execution capsule that intercepts database transactions on a reference level (`&DatabaseTransaction`).

The wrapper captures the return state of your handlers to enforce automatic actions:
* **`Ok(T)`:** Automatically registers a transaction **Commit** order.
* **`Err(E)`:** Automatically registers an immediate resource **Rollback** order, mitigating connection leaks.

```rust
#[endpoint(POST, "/v1/transact")]
async fn process_ledger_record(
    Db(db): Db,
    Json(payload): Json<LedgerPayload>
) -> Result<String, DomainError> {
    
    // Explicitly manages transaction lifetimes within clean Rust closure scopes
    db.transaction(|tx| async move {
        // All mutations apply directly to the transaction reference borrow context `tx`
        user_balance::Entity::update_many()
            .col_expr(user_balance::Column::Amount, Expr::value(payload.amount))
            .exec(tx).await?;

        Ok("Ledger transaction processed atomically".to_string())
    }).await
}
```

### 5. Automated Self-Documenting OpenAPI & Graphical Swagger UI
Kasi translates metadata tags from your attributes at compile time into internal documentation dictionaries. When the framework mounts your server, it automatically hosts a serialized contract document at `/openapi.json` and a fully interactive documentation panel at `/docs`.

```rust
#[endpoint(GET, "/v1/metrics")]
// The macro parses these attributes to compile compliance matrices at application boot time
#[response(status = 200, description = "Analytics payload computed successfully", json = MetricsReport)]
#[response(status = 401, description = "Invalid or expired security signature profile")]
async fn fetch_metrics() -> Result<MetricsReport, DomainError> {
    todo!()
}
```

---

## High-Fidelity Test Simulator

Kasi provides an in-memory testing apparatus leveraging virtual `tokio::io::duplex` streams. This allows developers to pass complete HTTP payloads, headers, OIDC credentials, and database contexts end-to-end through their production dispatch layers without allocating operating system loopback adapters or binding physical network sockets.
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use kasi::prelude::testing::TestStruct;
    use http::{Method, StatusCode};
    #[tokio::test]
    async fn test_microservice_pipeline_integration_end_to_end() {
        // Instantiate the application routing grid and inject state extensions
        let config = Config { host: "127.0.0.1".into(), port: 8080 };
        let mock_db = sea_orm::Database::connect("mirror://mock").await.unwrap();
        let app = Application::builder(config)
        .route(fetch_metrics)
        .extension(mock_db)
        .build().await.unwrap();
        // Bind the application layer directly to the in-memory test pipeline simulator
        let test_simulator = TestStruct::new(app);
        // Dispatch a high-fidelity client simulation request
        let http_response = test_simulator
        .simulate_request(Method::GET, "/v1/metrics")
        .await;
        // Compile standard assert configurations
        assert_eq!(http_response.status(), StatusCode::UNAUTHORIZED); // Fails correctly due to missing credentials header!
    }
}
```

---

## 🛠 Features & Use Cases Deep Dive

### 1. Advanced Declarative Struct Validation & Primitives
Validate incoming network data directly at the parameter level. By implementing a custom `#[derive(Validate)]` macro that matches structural bounds and compiles regex targets to lazy global slots, incorrect formats are safely dropped with an RFC 7807 problem details response before the handler execution task can allocate unnecessary resources.

```rust
use kasi_jb::prelude::*;
use serde::Deserialize;

#[derive(Deserialize, Validate)]
pub struct AccountUpdate {
    // Requires a safe string with minimum length of 3 matching alpha-numeric parameters
    #[validate(min_len = 3, regex = "^[a-z0-9_-]+\$")]
    pub username: String,
    
    // Automatic numeric range clamping boundaries
    #[validate(range(min = 18, max = 120))]
    pub age: u8,
}

#[endpoint(POST, "/accounts/modify")]
async fn modify_account(
    ValidatedJson(payload): ValidatedJson<AccountUpdate>
) -> Result<&'static str, Error> {
    println!("Processing payload safely for user: {}", payload.username);
    Ok("Account changes executed cleanly.")
}
```

### 2. High-Performance URL-Safe Regex Routing
Enforce path argument type constraints natively at the routing engine edge. Requests that fail to match the custom regex boundaries (such as a string instead of digits) drop out instantly into a clean `404 Not Found` without contaminating backend business scopes.

```rust
use kasi_jb::prelude::*;

// Matches exactly: GET /items/XYZ-98765
// Bypasses and emits 404 on: GET /items/abc-123 or GET /items/XYZ-
#[endpoint(GET, "/items/{sku:[A-Z]{3}-\\d+}")]
async fn get_inventory_item(Path(sku): Path<String>) -> Result<JsonValue, Error> {
    Ok(json!({
        "sku_serial": sku,
        "active_monitored": true
    }))
}

// Matches numeric IDs only: GET /users/45231
#[endpoint(GET, "/users/{id:\\d+}")]
async fn get_user_profile(Path(id): Path<u64>) -> Result<JsonValue, Error> {
    Ok(json!({ "user_id": id, "permissions": ["User"] }))
}
```

### 3. Automatic Atomic Database Transaction Extractor
Write code that handles transactions without manual rollback boilerplate. The `Tx` extractor automatically opens a database transaction boundary before your function runs. Success (`Ok`) triggers an automatic `COMMIT`, while an intercepted failure (`Err`) issues an atomic `ROLLBACK` instantly.

```rust
use kasi_jb::prelude::*;
use sea_orm::ActiveModelTrait;

#[endpoint(POST, "/checkout")]
async fn finalize_checkout(
    mut tx: Tx, // Automatically begins transactional boundaries on execution start
    ValidatedJson(order): ValidatedJson<OrderPayload>
) -> Result<&'static str, Error> {
    
    // Execute isolated mutation sets against the active database transaction context
    let record = invoice::ActiveModel { .. };
    record.insert(&tx.0).await.map_err(|e| Error::Internal(e.to_string()))?;

    if order.amount > 50000 {
        // Triggering an Err variant immediately commands an automatic rollback across all inserts
        return Err(Error::BadRequest("Exceeded maximum unverified batch transaction limit.".into()));
    }

    // Returning an Ok status cleanly issues an automatic COMMIT to the storage pool
    Ok("Checkout process locked and synchronized successfully.")
}
```

### 4. Hierarchical Route Tree Scope Nesting & Middleware Isolation
Compose massive API networks cleanly while maintaining strict middleware sub-tree boundaries. Parent-level middleware (like an authentication gate or audit logger) can be applied to an entire nested module group, protecting those endpoints without leaking headers or logic into parallel root scopes.

```rust
use kasi_jb::prelude::*;

#[main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Build an isolated subsystem child module with specialized routing paths
    let inventory_api = Module::new("/inventory")
        .add_endpoint_with_meta(
            Method::POST, 
            "/restock", 
            __register_restock_endpoint,
            EndpointMetadata::new().with_summary("Restock warehouse units")
        );

    // 2. Build a top-level route tree group, attach a middleware layer, and nest children
    // Paths automatically merge and compile down natively to: /api/v1/inventory/restock
    let api_cluster = Module::new("/api/v1")
        .wrap(from_fn(oauth2_jwt_auth_guard)) // Applies strictly to this tree branch
        .nest_with_docs(inventory_api);

    let app = Application::builder(Config { host: "127.0.0.1".into(), port: 8080 })
        .register_module(api_cluster) // Ingests handlers, custom middleware, and OpenAPI specs at once
        .build()
        .await?;

    app.serve().await?;
    Ok(())
}
```

### 5. Automated OpenAPI 3.1.0 Specs & Visual Swagger Dashboards
Kasi reads macro attributes compiled during builds and synthesizes an OpenAPI documentation schema at startup. The framework provisions static handlers internally to serve an interactive graphical dashboard out of the box, without requiring heavy third-party code generation suites.

```rust
// Access your microservice metadata dynamically via standard HTTP routing lookups:
// 🗺 JSON OpenApi Document Schema: http://127.0.0
// 📊 Graphical Interactive Swagger UI: http://127.0.0
```

---

## 🔬 Verified Integration Testing Infrastructure

To guarantee code durability across asynchronous worker threads, Kasi provides an eager socket-binding flow (`Application::bind_listener`) specifically designed for deterministic testing. By passing `port: 0` inside tests, the framework instructs the operating system kernel to bind an anonymous unallocated port, resolving local address strings accurately before executing client assertions.

```rust
#[cfg(test)]
mod tests {
    use kasi_jb::prelude::*;
    use http::StatusCode;

    #[tokio::test]
    async fn test_end_to_end_validation_and_regex_routing() {
        // A. Setup server context on an isolated ephemeral kernel socket
        let mut app = Application::builder(Config { host: "127.0.0.1".into(), port: 0 })
            .route(__register_get_inventory_item)
            .build()
            .await
            .unwrap();

        // B. Bind listener eagerly to fetch true active runtime port information
        let listener = app.bind_listener().await.unwrap();
        let target_addr = app.local_addr();

        // C. Spawn server loop on a background task worker
        tokio::task::spawn(async move {
            let _ = app.serve_on_listener(listener).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // D. Dispatch a request through reqwest client pipelines
        let client = reqwest::Client::new();
        let response = client.get(&format!("http://{}/items/ABC-12345", target_addr))
            .send()
            .await
            .unwrap();

        // E. Confirm that regex path match and validation processing execute flawlessly!
        assert_eq!(response.status(), StatusCode::OK);
    }
}
```
