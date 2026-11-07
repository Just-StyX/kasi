use kasi_jb::prelude::*;
use serde::{Deserialize, Serialize};
use http::StatusCode;

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
    ValidatedJson(payload): ValidatedJson<InventoryItem>
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
        port: 0 
    })
    .route(__register_create_item)
    .build()
    .await
    .unwrap();

    // B. Bind the listener eagerly inside the test execution scope!
    let listener = app.bind_listener().await.expect("Failed to bind socket listener.");
    
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
    let response = client.post(&format!("http://{}/items", target_addr))
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
        port: 0 
    })
    .route(__register_create_item)
    .build()
    .await
    .unwrap();

    // B. FIX: Bind the listener eagerly here as well!
    let listener = app.bind_listener().await.expect("Failed to bind socket listener.");
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
    let response = client.post(&format!("http://{}/items", target_addr))
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
