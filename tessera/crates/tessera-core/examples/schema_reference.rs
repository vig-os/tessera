//! Render the **built-in product-schema registry** as the markdown reference the book ships (#389).
//!
//! The registry is the source of truth, so the reference is *generated from it* rather than written
//! alongside it: `docs/book/src/schemas.md` wraps this output in a `guardrails:derived` region, and the
//! `derived-docs` gate fails the commit if the two disagree. A schema cannot gain a required field
//! without the documented table gaining it in the same commit.
//!
//! Regenerate the committed copy with:
//!
//! ```text
//! cd tessera && cargo run -q -p tessera-core --example schema_reference \
//!   > tests/derived-docs/schema-reference.md
//! ```
//!
//! `schema_reference_matches_the_committed_copy` (in `schema.rs`) fails if the two drift, and the book
//! `{{#include}}`s that one file — so the registry, the test and the published page cannot disagree.
//!
//! Why a test rather than the `derived-docs` gate: that gate runs inside the `guardrails-gates` nix
//! sandbox, which has **no cargo on PATH**, so a cargo-backed marker command can never succeed there.
//! A test runs where cargo obviously exists.

use tessera_core::schema::{FieldSpec, ProductSchema, SchemaRegistry};

/// The severity marker used in the field tables — the same three tiers `tessera schema` prints, so the
/// book and the CLI agree.
fn tier(f: &FieldSpec) -> &'static str {
    if f.required {
        "**required**"
    } else if f.recommended {
        "recommended"
    } else {
        "optional"
    }
}

/// Escape the pipe character, which would otherwise split a markdown table cell.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

fn field_rows(schema: &ProductSchema) -> String {
    if schema.fields.is_empty() {
        return "_No schema metadata fields._\n".to_string();
    }
    let mut out = String::from(
        "| field | tier | dtype | unit | sensitivity | description |\n\
         | --- | --- | --- | --- | --- | --- |\n",
    );
    for f in &schema.fields {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {:?} | {} |\n",
            cell(&f.id),
            tier(f),
            cell(&f.dtype),
            f.unit.as_deref().map_or("—".to_string(), cell),
            f.sensitivity,
            cell(&f.description),
        ));
    }
    out
}

fn block_rows(schema: &ProductSchema) -> String {
    if schema.blocks.is_empty() {
        return "_No required blocks._\n".to_string();
    }
    let mut out =
        String::from("| block role | kind | min | description |\n| --- | --- | --- | --- |\n");
    for b in &schema.blocks {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            cell(&b.role),
            b.kind
                .map_or("array or table".to_string(), |k| format!("{k:?}")
                    .to_lowercase()),
            b.min_count,
            cell(&b.description),
        ));
    }
    out
}

fn main() {
    let reg = SchemaRegistry::builtin();
    let mut products: Vec<&str> = reg.products().collect();
    products.sort_unstable();

    println!("<!-- Generated from the built-in SchemaRegistry — do not edit by hand. -->");
    println!();
    println!("| product | version | requires a recipe | description |");
    println!("| --- | --- | --- | --- |");
    for p in &products {
        if let Some(s) = reg.get(p) {
            println!(
                "| [`{}`](#{}) | {} | {} | {} |",
                cell(&s.product),
                s.product.replace('_', "-"),
                cell(&s.version),
                if s.requires_generation { "yes" } else { "no" },
                cell(&s.description),
            );
        }
    }

    for p in &products {
        let Some(s) = reg.get(p) else { continue };
        println!();
        println!("### {}", s.product);
        println!();
        println!("{}", s.description);
        println!();
        if s.requires_generation {
            println!(
                "Requires a **generation record** (ADR-0058 §3): a product of this kind must say how it \
                 was made, and sealing without one is a hard error."
            );
            println!();
        }
        print!("{}", field_rows(s));
        println!();
        print!("{}", block_rows(s));
    }
}
