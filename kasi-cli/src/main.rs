use std::env;
use std::fs;
use std::path::Path;
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "new" => {
            if args.len() < 3 {
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
            println!("  GET  http://127.0.0");
            println!("  GET  http://127.0.0health");
            println!("  GET  http://127.0.0docs");
        }
        "migrate" => {
            if args.len() < 3 {
                eprintln!("Usage: kasi migrate <create <name> | up | revert | info>");
                process::exit(1);
            }
            handle_migrate(&args[2..]);
        }
        _ => {
            print_usage();
            process::exit(1);
        }
    }
}

fn handle_migrate(sub_args: &[String]) {
    match sub_args[0].as_str() {
        "create" => {
            if sub_args.len() < 2 {
                eprintln!("Usage: kasi migrate create <migration_name>");
                process::exit(1);
            }
            let name = sub_args[1].replace(' ', "_").to_lowercase();
            let dir = Path::new("migrations");
            fs::create_dir_all(dir).expect("Failed to create migrations directory");

            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let filename = format!("{}_{}.sql", ts, name);
            let filepath = dir.join(&filename);

            let template = format!(
                "-- Migration: {}\n-- Created at: {}\n\n-- Write your SQL schema changes below:\n",
                name, ts
            );
            fs::write(&filepath, template).expect("Failed to write migration file");
            println!("Created migration: {}", filepath.display());
        }
        cmd @ ("up" | "revert" | "info") => {
            let status = Command::new("sqlx").args(["migrate", cmd]).status();

            match status {
                Ok(s) if s.success() => {}
                Ok(s) => process::exit(s.code().unwrap_or(1)),
                Err(_) => {
                    eprintln!("Error: `sqlx-cli` is not installed.");
                    eprintln!(
                        "Install it with: cargo install sqlx-cli --no-default-features --features postgres"
                    );
                    process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("Unknown migrate subcommand: {}", sub_args[0]);
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("Kasi Framework CLI");
    eprintln!("\nUsage:");
    eprintln!("  kasi new <project-name>          Scaffold a new Kasi microservice");
    eprintln!("  kasi migrate create <name>       Create a new timestamped SQL migration");
    eprintln!("  kasi migrate up                  Apply pending migrations via sqlx-cli");
    eprintln!("  kasi migrate revert              Revert the last applied migration");
    eprintln!("  kasi migrate info                List applied and pending migrations");
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
