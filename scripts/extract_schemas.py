"""Extract JSON Schema files from Python product schemas.

Generates standalone schema files into ``schemas/`` as the
language-agnostic single source of truth for fd5 file validation.

Usage::

    uv run python scripts/extract_schemas.py
"""

from __future__ import annotations

import json
from pathlib import Path

from fd5.registry import get_schema, list_schemas


def main() -> None:
    schemas_dir = Path(__file__).resolve().parent.parent / "schemas"
    schemas_dir.mkdir(exist_ok=True)

    manifest: dict[str, dict] = {}

    for product_type in sorted(list_schemas()):
        schema = get_schema(product_type)
        schema_dict = schema.json_schema()

        # Sanitise product_type for filename (e.g. "device_data" stays,
        # but hypothetical "foo/bar" becomes "foo_bar")
        safe_name = product_type.replace("/", "_")
        schema_file = f"{safe_name}.schema.json"
        out_path = schemas_dir / schema_file

        with open(out_path, "w") as f:
            json.dump(schema_dict, f, indent=2, sort_keys=False)
            f.write("\n")

        manifest[product_type] = {
            "schema_file": schema_file,
            "schema_version": schema.schema_version,
            "id_inputs": schema.id_inputs(),
            "required_root_attrs": schema.required_root_attrs(),
        }

        print(f"  {schema_file}")

    manifest_path = schemas_dir / "_manifest.json"
    with open(manifest_path, "w") as f:
        json.dump(manifest, f, indent=2, sort_keys=True)
        f.write("\n")

    print(f"\nWrote {len(manifest)} schemas + _manifest.json to {schemas_dir}")


if __name__ == "__main__":
    main()
