# fd5 Examples

Create a sealed, FAIR HDF5 file in any language. Each example:
1. Creates a builder with required metadata (product, name, description, timestamp)
2. Writes datasets with inline SHA-256 hashing
3. Seals the file (Merkle-tree content_hash + atomic rename)
4. Verifies the sealed file's integrity

## Quick Start (inside devcontainer)

```bash
just try-python   # Python — uses uv
just try-rust     # Rust   — uses cargo
just try-c        # C      — uses cmake + make
just try-all      # all 3
```

## Manual

**Python:**

```bash
uv run python examples/python/create_fd5.py
```

**Rust:**

```bash
cargo run --manifest-path examples/rust/Cargo.toml
```

**C:**

```bash
cd examples/c && mkdir -p build && cd build
cmake .. && make && ./create_fd5
```
