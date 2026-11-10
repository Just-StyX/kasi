use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EndpointMetadata {
    pub summary: Option<String>,
    pub responses: HashMap<String, ResponseSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseSpec {
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<HashMap<String, MediaTypeSpec>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaTypeSpec {
    pub schema: SchemaRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaRef {
    #[serde(rename = "$ref")]
    pub reference: String,
}

/// Represents an OpenAPI 3.1.0 Schema definition for a struct or field.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchemaObject {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub schema_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, SchemaObject>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<SchemaObject>>,
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// Container for reusable OpenAPI components.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ComponentsSpec {
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub schemas: HashMap<String, SchemaObject>,
}

/// Trait implemented by `#[derive(Schema)]` to generate OpenAPI JSON schemas.
pub trait Schema {
    fn schema_name() -> &'static str;
    fn schema_object() -> SchemaObject;
}

impl EndpointMetadata {
    pub fn new() -> Self {
        Self {
            summary: None,
            responses: HashMap::new(),
        }
    }

    pub fn add_response(&mut self, status: u16, description: &str, json_model: Option<&str>) {
        let content = json_model.map(|model| {
            let mut media_types = HashMap::new();
            media_types.insert(
                "application/json".to_string(),
                MediaTypeSpec {
                    schema: SchemaRef {
                        reference: format!("#/components/schemas/{}", model),
                    },
                },
            );
            media_types
        });

        self.responses.insert(
            status.to_string(),
            ResponseSpec {
                description: description.to_string(),
                content,
            },
        );
    }
}

// Global representation of the final generated Swagger Spec Sheet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiSpec {
    pub openapi: String,
    pub info: InfoSpec,
    pub paths: HashMap<String, HashMap<String, EndpointMetadata>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<ComponentsSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoSpec {
    pub title: String,
    pub version: String,
}
