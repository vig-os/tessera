//! fd5 attribute editing with copy-on-write or in-place modes.
//!
//! After modifying an attribute, the `content_hash` is recomputed and
//! written back, re-sealing the file.

use std::path::{Path, PathBuf};

use hdf5_metno::types::VarLenUnicode;
use hdf5_metno::File;

use crate::error::Fd5Result;
use crate::hash::compute_content_hash;

/// How the edit should be applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditMode {
    /// Copy the file first, edit the copy (safe default).
    CopyOnWrite,
    /// Edit the original file in place (dev/expert flag).
    InPlace,
}

/// Typed attribute values for writing.
#[derive(Debug, Clone)]
pub enum AttrValue {
    String(String),
    Int64(i64),
    Float64(f64),
}

/// Description of a planned edit — shown in confirmation dialog before applying.
#[derive(Debug, Clone)]
pub struct EditPlan {
    pub source_path: PathBuf,
    pub attr_path: String,
    pub attr_name: String,
    pub old_value: String,
    pub new_value: AttrValue,
    pub mode: EditMode,
}

/// Result of a completed edit.
#[derive(Debug, Clone)]
pub struct EditResult {
    pub output_path: PathBuf,
    pub old_content_hash: String,
    pub new_content_hash: String,
}

fn make_vlu(s: &str) -> VarLenUnicode {
    s.parse().expect("content_hash should not contain null bytes")
}

impl EditPlan {
    /// Apply the edit plan: modify the attribute and re-seal with new content_hash.
    pub fn apply(&self) -> Fd5Result<EditResult> {
        let target_path = match self.mode {
            EditMode::CopyOnWrite => {
                let stem = self
                    .source_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file");
                let ext = self
                    .source_path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("h5");
                let parent = self.source_path.parent().unwrap_or(Path::new("."));
                let target = parent.join(format!("{}_edited.{}", stem, ext));
                std::fs::copy(&self.source_path, &target)?;
                target
            }
            EditMode::InPlace => self.source_path.clone(),
        };

        // Open for read-write
        let file = File::open_rw(&target_path)?;
        let root_group: &hdf5_metno::Group = &*file;

        // Read old content_hash
        let old_hash = root_group
            .attr("content_hash")
            .ok()
            .and_then(|a| {
                a.read_scalar::<VarLenUnicode>()
                    .map(|v| v.as_str().to_string())
                    .ok()
            })
            .unwrap_or_default();

        // Write the new attribute value on the target object
        if self.attr_path == "/" {
            write_attr(root_group, &self.attr_name, &self.new_value)?;
        } else {
            let target_group = root_group.group(&self.attr_path)?;
            write_attr(&target_group, &self.attr_name, &self.new_value)?;
        }

        // Recompute and write new content_hash
        let new_hash = compute_content_hash(&file)?;
        // Delete old content_hash and write new
        if root_group.attr("content_hash").is_ok() {
            root_group.delete_attr("content_hash")?;
        }
        let vlu = make_vlu(&new_hash);
        root_group
            .new_attr::<VarLenUnicode>()
            .shape(())
            .create("content_hash")?
            .write_scalar(&vlu)?;

        file.flush()?;

        Ok(EditResult {
            output_path: target_path,
            old_content_hash: old_hash,
            new_content_hash: new_hash,
        })
    }
}

/// Write a typed value as an HDF5 attribute, replacing any existing attribute.
fn write_attr(
    loc: &hdf5_metno::Location,
    name: &str,
    value: &AttrValue,
) -> Fd5Result<()> {
    // Delete existing attribute if present
    if loc.attr(name).is_ok() {
        loc.delete_attr(name)?;
    }

    match value {
        AttrValue::String(s) => {
            let vlu = make_vlu(s);
            loc.new_attr::<VarLenUnicode>()
                .shape(())
                .create(name)?
                .write_scalar(&vlu)?;
        }
        AttrValue::Int64(v) => {
            loc.new_attr::<i64>().shape(()).create(name)?.write_scalar(v)?;
        }
        AttrValue::Float64(v) => {
            loc.new_attr::<f64>().shape(()).create(name)?.write_scalar(v)?;
        }
    }
    Ok(())
}
