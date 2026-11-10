use std::env;
use std::fs;
use std::path::Path;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 || args[1] != "new" {
        eprintln!("Usage: kasi new <project-name>");
        process::exit(1);
    }

    let project_name = &args[2];
    let root = Path::new(project_name);

    if root.exists() {
        eprintln!("Error: Directory '{}' already exists.", project_name);
        process::exit(1);
    }

    if let Err(err) = scaffold_project(root, project_name) {
        eprintln!("Failed to create project: {}", err);
        process::exit(1);
    }

    println!("Created new Kasi microservice in ./{}", project_name);
    println!("\nNext steps:");
    println!("  cd {}", project_name);
    println!("  cargo run");
    println!("\nEndpoints ready:");
    println!("  GET  http://127.0.0.1");
    println!("  GET  http://127.0.0.1/health");
    println!("  GET  http://127.0.0.1/docs");
}

fn scaffold_project(root: &Path, name: &str) -> std::io::Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("templates"))?;
    fs::create_dir_all(root.join("public"))?;

    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.3.0"
edition = "2024"

[dependencies]
kasi-jb = {{ version = "0.2", default-features = false, features = ["templates"] }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
"#
    );

    let main_rs = r#"use kasi_jb::prelude::*;

#[derive(Serialize, IntoResponse, Schema)]
struct HealthStatus {
    status: String,
    service: String,
}

#[endpoint(GET, "/")]
async fn index(engine: TemplateEngine) -> Result<Html, Error> {
    engine.render("index.html", template_context!(title => "Kasi Microservice"))
}

#[endpoint(GET, "/health")]
#[response(status = 200, description = "Service health check", json = HealthStatus)]
async fn health() -> HealthStatus {
    HealthStatus {
        status: "ok".into(),
        service: env!("CARGO_PKG_NAME").into(),
    }
}

#[main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let templates = TemplateEngine::from_dir("templates");

    let app = Application::builder(Config {
        host: "127.0.0.1".into(),
        port: 8080,
    })
    .middleware(TracingMiddleware::new())
    .extension(templates)
    .serve_dir("/static", "public")
    .route(index)
    .route(health)
    .build()
    .await?;

    app.serve().await?;
    Ok(())
}
"#;

    let index_html = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8" />
    <title>{{ title }}</title>
    <link rel="stylesheet" href="/static/style.css" />
</head>
<body>
    <main>
        <h1>{{ title }} is running!</h1>
        <p>Explore your API documentation at <a href="/docs">/docs</a>.</p>
    </main>
</body>
</html>
"#;

    let style_css = r#"body {
    font-family: system-ui, -apple-system, sans-serif;
    max-width: 640px;
    margin: 4rem auto;
    padding: 0 1rem;
    color: #1a1a1a;
}
a { color: #0066cc; }
"#;

    fs::write(root.join("Cargo.toml"), cargo_toml)?;
    fs::write(root.join("src/main.rs"), main_rs)?;
    fs::write(root.join("templates/index.html"), index_html)?;
    fs::write(root.join("public/style.css"), style_css)?;

    Ok(())
}
