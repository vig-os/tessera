"""Create a sealed fd5 recon file — complete working example.

Usage:
    uv run python examples/python/create_fd5.py
"""

import numpy as np

import fd5

with fd5.create(
    "/tmp/fd5-examples",
    product="recon",
    name="example-brain-pet",
    description="Synthetic FDG-PET brain reconstruction for fd5 demo",
    timestamp="2026-01-01T12:00:00Z",
) as b:
    # Required root attrs for recon schema
    b.file.attrs["domain"] = "medical_imaging"
    b.file.attrs["scanner"] = "Example PET/CT"
    b.file.attrs["vendor_series_id"] = "demo-001"

    # Write a 3D brain volume through the recon product schema
    rng = np.random.default_rng(42)
    volume = rng.standard_normal((64, 128, 128)).astype(np.float32)

    b.write_product(
        {
            "volume": volume,
            "affine": np.eye(4),
            "dimension_order": "ZYX",
            "reference_frame": "LPS",
            "description": "Synthetic FDG-PET brain volume",
        }
    )

    # Write arbitrary metadata (nested dict → HDF5 groups/attrs)
    b.write_metadata(
        {
            "institution": "fd5 Lab",
            "tracer": "FDG",
            "injection_dose_MBq": 370.0,
        }
    )

# The file is now sealed with content_hash — find it
path = next(b._out_dir.glob("*.h5"))
print(f"Created  : {path}")
print(f"Size     : {path.stat().st_size / 1024:.0f} KB")

# Verify integrity (recomputes Merkle tree, compares with stored hash)
ok = fd5.verify(str(path))
print(f"Verified : {'PASS' if ok else 'FAIL'}")
