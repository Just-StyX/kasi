use http::StatusCode;
use kasi_jb::prelude::*;
use serde::{Deserialize, Serialize};

// ==========================================
// 1. MOCK DOMAIN STRUCTS FOR VERIFICATION
// ==========================================

#[derive(Debug, Serialize, Deserialize, Validate)]
struct InventoryItem {
    #[validate(min_len = 5)]
    pub sku: String,

    #[validate(range(min = 1, max = 500))]
    pub quantity: i32,
}

// ==========================================
// 2. MOCK ENDPOINTS USING FRAMEWORK MACROS
// ==========================================

#[endpoint(POST, "/items")]
async fn create_item(
    ValidatedJson(payload): ValidatedJson<InventoryItem>,
) -> Result<&'static str, Error> {
    println!("{:#?}", payload);
    Ok("Item verified and saved successfully.")
}

// ==========================================
// 3. AUTOMATED SYSTEM INTEGRATION SUITE
// ==========================================

#[tokio::test]
async fn test_validation_failure_emits_rfc7807_problem_details() {
    // A. Initialize the application container configuration
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .route(create_item)
    .build()
    .await
    .unwrap();

    // B. Bind the listener eagerly inside the test execution scope!
    let listener = app
        .bind_listener()
        .await
        .expect("Failed to bind socket listener.");

    // This now safely returns the true active port assigned by the OS (e.g. 127.0.0.1:40715)
    let target_addr = app.local_addr();

    // C. Hand ownership of the bound listener over to the execution task
    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // D. Construct a deliberately bad payload violating constraints
    let bad_payload = serde_json::json!({ "sku": "ABC", "quantity": 999 });
    let client = reqwest::Client::new();
    let response = client
        .post(&format!("http://{}/items", target_addr))
        .json(&bad_payload)
        .send()
        .await
        .unwrap();

    // E. Assert that our validation layer correctly short-circuits to a 400 Bad Request
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_validation_success_passes_cleanly() {
    // A. Re-initialize application stack state
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .route(create_item)
    .build()
    .await
    .unwrap();

    // B. FIX: Bind the listener eagerly here as well!
    let listener = app
        .bind_listener()
        .await
        .expect("Failed to bind socket listener.");
    let target_addr = app.local_addr();

    // C. FIX: Hand ownership of the pre-bound listener to the runtime worker task
    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // D. Build a valid dataset compliant with all macro parameters
    let good_payload = serde_json::json!({
        "sku": "PROD-10042",
        "quantity": 150
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&format!("http://{}/items", target_addr))
        .header("Content-Type", "application/json")
        .json(&good_payload)
        .send()
        .await
        .unwrap();

    // E. Validation passes cleanly, handoff completes successfully returning 200 OK
    assert_eq!(response.status(), StatusCode::OK);

    let text_output = response.text().await.unwrap();
    assert_eq!(text_output, "Item verified and saved successfully.");
}

// ==========================================
// 1. DATA STRUCT MODEL
// ==========================================
#[derive(Debug, Serialize, Deserialize)]
struct RegexItemResponse {
    pub sku: String,
    pub verified: bool,
}

// ==========================================
// 2. REGEX-CONSTRAINED ROUTE ENDPOINT
// ==========================================

// Matches exactly three uppercase letters, a hyphen, and one or more digits (e.g., ABC-123)
#[endpoint(GET, "/items/{sku:[A-Z]{3}-\\d+}")]
async fn get_item_by_regex_sku(Path(sku): Path<String>) -> Result<JsonValue, Error> {
    Ok(json!(RegexItemResponse {
        sku,
        verified: true,
    }))
}

// ==========================================
// 3. AUTOMATED REGEX INTEGRATION TESTS
// ==========================================

#[tokio::test]
async fn test_regex_route_matching_success() {
    // A. Initialize the application container configuration
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .route(get_item_by_regex_sku)
    .build()
    .await
    .unwrap();

    // B. Bind the socket listener eagerly to catch the dynamic OS port
    let listener = app
        .bind_listener()
        .await
        .expect("Failed to bind socket listener.");
    let target_addr = app.local_addr();

    // C. Spawn the runtime loop on a background task
    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // D. Dispatch a compliant SKU parameter: ABC-12345
    let client = reqwest::Client::new();
    let response = client
        .get(&format!("http://{}/items/ABC-12345", target_addr))
        .send()
        .await
        .unwrap();

    // E. Assert route resolves cleanly to a 200 OK
    assert_eq!(response.status(), StatusCode::OK);

    // F. Verify the returned JSON payload parsed the parameter correctly
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body.get("sku").unwrap().as_str().unwrap(), "ABC-12345");
    assert_eq!(body.get("verified").unwrap().as_bool().unwrap(), true);
}

#[tokio::test]
async fn test_regex_route_matching_rejection_falls_through_to_404() {
    // A. Re-initialize the application container configuration
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .route(get_item_by_regex_sku)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // Case 1: Lowercase letters (abc-123) should fail regex constraints
    let res_lowercase = client
        .get(&format!("http://{}/items/abc-123", target_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res_lowercase.status(), StatusCode::NOT_FOUND);

    // Case 2: Wrong count of characters (AB-123) should fail regex constraints
    let res_short = client
        .get(&format!("http://{}/items/AB-123", target_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res_short.status(), StatusCode::NOT_FOUND);

    // Case 3: Missing trailing numerical components (ABC-) should fail regex constraints
    let res_no_digits = client
        .get(&format!("http://{}/items/ABC-", target_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res_no_digits.status(), StatusCode::NOT_FOUND);
}

// ==========================================
// 4. TRACING & NESTED MODULE INTEGRATION TESTS
// ==========================================

#[derive(Debug, Serialize, Deserialize, Validate)]
struct RestockRequest {
    #[validate(range(min = 1, max = 1000))]
    pub units: i32,
}

#[endpoint(POST, "/restock")]
async fn restock_inventory(
    ctx: RequestContext,
    ValidatedJson(payload): ValidatedJson<RestockRequest>,
) -> Result<JsonValue, Error> {
    Ok(json!({
        "restocked": payload.units,
        "trace_id": ctx.trace_id().to_string(),
        "path": ctx.uri_path(),
    }))
}

#[tokio::test]
async fn test_tracing_propagation_and_nested_module_routing() {
    // 1. Build child module and nest inside `/api/v1` with custom middleware
    let inventory_mod = Module::new("/inventory").route(restock_inventory);

    let api_v1 = Module::new("/api/v1")
        .wrap(kasi_core::middleware::from_fn(|req, next| async move {
            let mut res = next.run(req).await;
            res.headers_mut()
                .insert("x-module-scope", HeaderValue::from_static("api-v1"));
            Ok(res)
        }))
        .nest_with_docs(inventory_mod);

    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .middleware(TracingMiddleware::new())
    .register_module(api_v1)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let custom_trace_id = uuid::Uuid::new_v4().to_string();

    // Case A: Valid request propagates custom X-Request-Id and parent module middleware header
    let ok_res = client
        .post(&format!("http://{}/api/v1/inventory/restock", target_addr))
        .header("x-request-id", &custom_trace_id)
        .json(&serde_json::json!({ "units": 50 }))
        .send()
        .await
        .unwrap();

    assert_eq!(ok_res.status(), StatusCode::OK);
    assert_eq!(
        ok_res
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap(),
        custom_trace_id
    );
    assert_eq!(
        ok_res
            .headers()
            .get("x-module-scope")
            .unwrap()
            .to_str()
            .unwrap(),
        "api-v1"
    );

    let ok_json: serde_json::Value = ok_res.json().await.unwrap();
    assert_eq!(ok_json["trace_id"], custom_trace_id);
    assert_eq!(ok_json["path"], "/api/v1/inventory/restock");

    // Case B: Validation failure embeds the active trace_id and real URI path in RFC 7807 JSON
    let err_res = client
        .post(&format!("http://{}/api/v1/inventory/restock", target_addr))
        .header("x-request-id", &custom_trace_id)
        .json(&serde_json::json!({ "units": -5 }))
        .send()
        .await
        .unwrap();

    assert_eq!(err_res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        err_res
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap(),
        custom_trace_id
    );

    let err_json: serde_json::Value = err_res.json().await.unwrap();
    assert_eq!(err_json["trace_id"], custom_trace_id);
    assert_eq!(err_json["instance"], "/api/v1/inventory/restock");
}

// ==========================================
// 5. OPENAPI SCHEMA, TEMPLATES & RATE LIMIT TESTS
// ==========================================

#[derive(Debug, Serialize, Deserialize, IntoResponse, Schema)]
struct ProductSummary {
    pub id: uuid::Uuid,
    pub name: String,
    pub price_cents: u32,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[endpoint(GET, "/products/{id}")]
#[response(status = 200, description = "Returns product details", json = ProductSummary)]
async fn get_product(Path(id): Path<uuid::Uuid>) -> Result<ProductSummary, Error> {
    Ok(ProductSummary {
        id,
        name: "Mechanical Keyboard".into(),
        price_cents: 14900,
        description: None,
        tags: vec!["hardware".into(), "peripherals".into()],
    })
}

#[tokio::test]
async fn test_openapi_schema_generation_endpoint() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .register_schema::<ProductSummary>()
    .route(get_product)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://{}/openapi.json", target_addr))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let spec: serde_json::Value = res.json().await.unwrap();

    // Verify path reference and generated component schema fields
    let schema = &spec["components"]["schemas"]["ProductSummary"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["id"]["format"], "uuid");
    assert_eq!(schema["properties"]["price_cents"]["type"], "integer");
    assert_eq!(schema["properties"]["tags"]["type"], "array");

    // `description` is Option<String>, so it must NOT appear in `required`
    let required = schema["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v == "id"));
    assert!(required.iter().any(|v| v == "name"));
    assert!(!required.iter().any(|v| v == "description"));
}

#[cfg(feature = "templates")]
#[endpoint(GET, "/hello/{name}")]
async fn render_hello_page(
    engine: TemplateEngine,
    Path(name): Path<String>,
) -> Result<Html, Error> {
    engine.render("hello.html", template_context!(name => name))
}

#[cfg(feature = "templates")]
#[tokio::test]
async fn test_minijinja_template_rendering() {
    let mut env = minijinja::Environment::new();
    env.add_template("hello.html", "<h1>Hello, {{ name }}!</h1>")
        .unwrap();
    let engine = TemplateEngine::from_env(env);

    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .extension(engine)
    .route(render_hello_page)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://{}/hello/Justice", target_addr))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("content-type").unwrap().to_str().unwrap(),
        "text/html; charset=utf-8"
    );
    assert_eq!(res.text().await.unwrap(), "<h1>Hello, Justice!</h1>");
}

#[endpoint(GET, "/ping")]
async fn ping_handler() -> &'static str {
    "pong"
}

#[tokio::test]
async fn test_rate_limit_middleware_enforces_quota() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .middleware(RateLimitMiddleware::new(
        2,
        std::time::Duration::from_secs(60),
    ))
    .route(ping_handler)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{}/ping", target_addr);

    // First 2 requests succeed
    let r1 = client.get(&url).send().await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    assert_eq!(r1.headers().get("x-ratelimit-remaining").unwrap(), "1");

    let r2 = client.get(&url).send().await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    assert_eq!(r2.headers().get("x-ratelimit-remaining").unwrap(), "0");

    // 3rd request is throttled with 429 Too Many Requests
    let r3 = client.get(&url).send().await.unwrap();
    assert_eq!(r3.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_serve_dir_and_path_traversal_protection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let temp_dir = std::env::temp_dir().join(format!("kasi_static_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    std::fs::write(temp_dir.join("app.css"), "body { color: red; }").unwrap();

    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .serve_dir("/static", &temp_dir)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // 1. Valid CSS asset request
    let ok_res = client
        .get(&format!("http://{}/static/app.css", target_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(ok_res.status(), StatusCode::OK);
    assert_eq!(
        ok_res
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/css; charset=utf-8"
    );
    assert_eq!(ok_res.text().await.unwrap(), "body { color: red; }");

    // 2. Path traversal attempt sent directly over TCP so the client doesn't normalize `..`
    let mut stream = tokio::net::TcpStream::connect(&target_addr).await.unwrap();
    let raw_http = format!(
        "GET /static/../Cargo.toml HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        target_addr
    );
    stream.write_all(raw_http.as_bytes()).await.unwrap();

    let mut raw_response = String::new();
    stream.read_to_string(&mut raw_response).await.unwrap();

    assert!(
        raw_response.starts_with("HTTP/1.1 403 Forbidden"),
        "Expected 403 Forbidden, got: {}",
        raw_response.lines().next().unwrap_or("")
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[endpoint(POST, "/upload")]
async fn upload_handler(form: Multipart) -> JsonValue {
    let title = form.fields.get("title").cloned().unwrap_or_default();
    let file_info: Vec<_> = form
        .files
        .iter()
        .map(|f| {
            json!({
                "field": f.field_name,
                "filename": f.file_name,
                "size": f.bytes.len(),
            })
        })
        .collect();

    json!({ "title": title, "files": file_info })
}

#[tokio::test]
async fn test_multipart_file_upload_extractor() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .route(upload_handler)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let boundary = "KasiTestBoundary123";
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nQuarterly Report\r\n\
         --{b}\r\nContent-Disposition: form-data; name=\"document\"; filename=\"report.txt\"\r\nContent-Type: text/plain\r\n\r\nHello Kasi!\r\n\
         --{b}--\r\n",
        b = boundary
    );

    let client = reqwest::Client::new();
    let res = client
        .post(&format!("http://{}/upload", target_addr))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(body)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let data: serde_json::Value = res.json().await.unwrap();
    assert_eq!(data["title"], "Quarterly Report");
    assert_eq!(data["files"][0]["filename"], "report.txt");
    assert_eq!(data["files"][0]["size"], 11);
}

#[endpoint(POST, "/login")]
async fn login_cookie_handler() -> Response<BoxBody> {
    let mut res = "Logged in".into_response();
    Cookie::new("session_id", "tok_998877")
        .http_only(true)
        .same_site(SameSite::Strict)
        .max_age(3600)
        .append_to_headers(res.headers_mut());
    res
}

#[endpoint(GET, "/me")]
async fn current_user_cookie(cookies: Cookies) -> JsonValue {
    json!({
        "session_id": cookies.get("session_id").unwrap_or("none")
    })
}

#[tokio::test]
async fn test_cookie_extraction_and_set_cookie_builder() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .route(login_cookie_handler)
    .route(current_user_cookie)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // 1. Login sets `Set-Cookie` header
    let login_res = client
        .post(&format!("http://{}/login", target_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(login_res.status(), StatusCode::OK);
    let set_cookie = login_res
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("session_id=tok_998877"));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));

    // 2. Subsequent request sends `Cookie` header and extracts it via `Cookies`
    let me_res = client
        .get(&format!("http://{}/me", target_addr))
        .header("Cookie", "session_id=tok_998877; theme=dark")
        .send()
        .await
        .unwrap();
    assert_eq!(me_res.status(), StatusCode::OK);
    let body: serde_json::Value = me_res.json().await.unwrap();
    assert_eq!(body["session_id"], "tok_998877");
}

#[derive(Debug, Serialize, Deserialize, Schema)]
struct CreateOrderDto {
    pub item_id: uuid::Uuid,
    pub quantity: u32,
}

#[endpoint(POST, "/orders/schema-check")]
#[response(status = 201, description = "Order created")]
async fn create_order_schema_endpoint(Json(body): Json<CreateOrderDto>) -> JsonValue {
    json!({ "item_id": body.item_id, "qty": body.quantity })
}

#[tokio::test]
async fn test_openapi_auto_request_body_schema() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .register_schema::<CreateOrderDto>()
    .route(create_order_schema_endpoint)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let spec: serde_json::Value = reqwest::Client::new()
        .get(&format!("http://{}/openapi.json", target_addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let req_body = &spec["paths"]["/orders/schema-check"]["post"]["requestBody"];
    assert_eq!(req_body["required"], true);
    assert_eq!(
        req_body["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/CreateOrderDto"
    );
}

#[tokio::test]
async fn test_background_task_scheduler() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .schedule_task(
        "heartbeat-counter",
        std::time::Duration::from_millis(30),
        move || {
            let c = counter_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        },
    )
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(110)).await;
    assert!(counter.load(Ordering::SeqCst) >= 2);
}

#[endpoint(GET, "/ws/echo")]
async fn ws_echo_handler(ws: WebSocketUpgrade) -> Response<BoxBody> {
    ws.on_upgrade(|mut socket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            if let WsMessage::Text(txt) = msg {
                let _ = socket.send_text(format!("echo: {}", txt)).await;
                break;
            }
        }
        let _ = socket.close().await;
    })
}

#[tokio::test]
async fn test_websocket_bidirectional_echo() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::connect_async;

    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .route(ws_echo_handler)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (mut ws_stream, _) = connect_async(format!("ws://{}/ws/echo", target_addr))
        .await
        .expect("Failed to connect WebSocket client");

    ws_stream
        .send(WsMessage::Text("hello kasi".into()))
        .await
        .unwrap();

    let reply = ws_stream.next().await.unwrap().unwrap();
    assert_eq!(reply.into_text().unwrap(), "echo: hello kasi");
}

// ==========================================
// 6. SSE, GZIP COMPRESSION & METRICS TESTS
// ==========================================

#[endpoint(GET, "/events/stream")]
async fn sse_test_stream() -> Sse {
    use futures_util::stream;
    use std::convert::Infallible;

    let events = vec![
        Ok::<SseEvent, Infallible>(SseEvent::data("first-msg").event("update").id("1")),
        Ok::<SseEvent, Infallible>(SseEvent::data("second-msg").event("update").id("2")),
    ];

    Sse::new(stream::iter(events))
}

#[tokio::test]
async fn test_server_sent_events_streaming() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .route(sse_test_stream)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://{}/events/stream", target_addr))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("content-type").unwrap().to_str().unwrap(),
        "text/event-stream"
    );

    let body_text = res.text().await.unwrap();
    assert!(body_text.contains("id: 1\nevent: update\ndata: first-msg\n\n"));
    assert!(body_text.contains("id: 2\nevent: update\ndata: second-msg\n\n"));
}

#[endpoint(GET, "/large-payload")]
async fn large_payload_handler() -> String {
    // Exceeds default 256-byte threshold so CompressionMiddleware applies gzip
    "Kasi framework response compression verification string. ".repeat(20)
}

#[tokio::test]
async fn test_gzip_response_compression_middleware() {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .middleware(CompressionMiddleware::new())
    .route(large_payload_handler)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let res = client
        .get(&format!("http://{}/large-payload", target_addr))
        .header("Accept-Encoding", "gzip")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get("content-encoding")
            .unwrap()
            .to_str()
            .unwrap(),
        "gzip"
    );

    let compressed_bytes = res.bytes().await.unwrap();
    let mut decoder = GzDecoder::new(&compressed_bytes[..]);
    let mut decompressed = String::new();
    decoder.read_to_string(&mut decompressed).unwrap();

    assert_eq!(
        decompressed,
        "Kasi framework response compression verification string. ".repeat(20)
    );
}

#[tokio::test]
async fn test_observability_healthz_and_prometheus_metrics() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .with_observability()
    .route(ping_handler)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // 1. Check /healthz liveness endpoint
    let health_res = client
        .get(&format!("http://{}/healthz", target_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(health_res.status(), StatusCode::OK);
    let health_json: serde_json::Value = health_res.json().await.unwrap();
    assert_eq!(health_json["status"], "healthy");

    // 2. Hit an application route so metrics increment
    let ping_res = client
        .get(&format!("http://{}/ping", target_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(ping_res.status(), StatusCode::OK);

    // 3. Scrape /metrics Prometheus text exposition endpoint
    let metrics_res = client
        .get(&format!("http://{}/metrics", target_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(metrics_res.status(), StatusCode::OK);

    let metrics_text = metrics_res.text().await.unwrap();
    assert!(metrics_text.contains("# TYPE kasi_http_requests_total counter"));
    assert!(metrics_text.contains("kasi_http_requests_total 3"));
    assert!(metrics_text.contains("kasi_http_responses_by_class_total{class=\"2xx\"} 2"));
    assert!(metrics_text.contains("kasi_http_in_flight_requests 1"));
}

#[endpoint(GET, "/slow")]
async fn slow_endpoint() -> &'static str {
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    "finished"
}

#[tokio::test]
async fn test_timeout_middleware_returns_408() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .middleware(TimeoutMiddleware::new(std::time::Duration::from_millis(30)))
    .route(slow_endpoint)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let res = reqwest::Client::new()
        .get(&format!("http://{}/slow", target_addr))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::REQUEST_TIMEOUT);
}

#[tokio::test]
async fn test_api_key_and_bearer_token_middlewares() {
    let mut app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .middleware(ApiKeyMiddleware::new("x-api-key", ["secret-key-123"]))
    .middleware(BearerTokenMiddleware::new(["svc-token-abc"]))
    .route(ping_handler)
    .build()
    .await
    .unwrap();

    let listener = app.bind_listener().await.unwrap();
    let target_addr = app.local_addr();

    tokio::task::spawn(async move {
        let _ = app.serve_on_listener(listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{}/ping", target_addr);

    // 1. Missing headers -> 401 Unauthorized
    let unauth = client.get(&url).send().await.unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    // 2. Valid API Key + Valid Bearer Token -> 200 OK
    let ok = client
        .get(&url)
        .header("x-api-key", "secret-key-123")
        .header("Authorization", "Bearer svc-token-abc")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
}
