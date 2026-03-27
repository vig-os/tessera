//! Product schema trait and registry.
//!
//! Each product type (e.g. `imaging/recon`, `imaging/sinogram`) has a schema
//! that knows how to write product-specific data and provides the JSON Schema
//! for validation.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::builder::HashTrackingGroup;
use crate::error::{Fd5Error, Fd5Result};

/// Trait that every product schema must implement.
pub trait ProductSchema: Send + Sync {
    /// The product type string (e.g. `"imaging/recon"`).
    fn product_type(&self) -> &str;

    /// The schema version string (e.g. `"1.0.0"`).
    fn schema_version(&self) -> &str;

    /// The JSON Schema as a `serde_json::Value`.
    fn json_schema(&self) -> Value;

    /// The list of root attribute keys used to compute the file id.
    fn id_inputs(&self) -> Vec<String>;

    /// Write product-specific data through the hash-tracking group.
    fn write(&self, target: &HashTrackingGroup, data: &Value) -> Fd5Result<()>;
}

// ---------------------------------------------------------------------------
// Global registry
// ---------------------------------------------------------------------------

static REGISTRY: Mutex<Option<HashMap<String, Box<dyn ProductSchema>>>> = Mutex::new(None);

fn with_registry<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<String, Box<dyn ProductSchema>>) -> R,
{
    let mut lock = REGISTRY.lock().unwrap();
    let map = lock.get_or_insert_with(HashMap::new);
    f(map)
}

/// Register a product schema. Overwrites any existing schema for the same product type.
pub fn register_schema(schema: Box<dyn ProductSchema>) {
    let key = schema.product_type().to_string();
    with_registry(|map| {
        map.insert(key, schema);
    });
}

/// Look up the schema for a product type. Returns an error if not found.
pub fn get_schema(product: &str) -> Fd5Result<Box<dyn ProductSchema>> {
    with_registry(|map| {
        map.remove(product).ok_or_else(|| {
            Fd5Error::Other(format!(
                "No schema registered for product type '{}'",
                product
            ))
        })
    })
}

// ---------------------------------------------------------------------------
// Test product schema
// ---------------------------------------------------------------------------

/// A simple test product schema for unit tests.
pub struct TestProductSchema;

impl ProductSchema for TestProductSchema {
    fn product_type(&self) -> &str {
        "test/product"
    }

    fn schema_version(&self) -> &str {
        "1.0.0"
    }

    fn json_schema(&self) -> Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "values": {"type": "array", "items": {"type": "number"}}
            }
        })
    }

    fn id_inputs(&self) -> Vec<String> {
        vec!["product".into(), "name".into(), "timestamp".into()]
    }

    fn write(&self, target: &HashTrackingGroup, data: &Value) -> Fd5Result<()> {
        if let Some(values) = data.get("values").and_then(|v| v.as_array()) {
            let floats: Vec<f64> = values
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0))
                .collect();
            target.create_dataset_f64("values", &floats)?;
        }
        Ok(())
    }
}
