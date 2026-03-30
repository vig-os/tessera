# Creating a Custom fd5 Product Schema

This tutorial walks through building a complete product schema from scratch,
so you can extend fd5 for any domain -- not just medical imaging.

By the end you will have a working `weather-station` schema that stores
temperature, humidity, pressure, and wind-speed time series in a
content-addressed, self-describing HDF5 file.

## 1. Overview

### What is a product schema?

Every fd5 file is typed by a **product schema**. The schema decides:

- what data the file must contain (JSON Schema validation),
- what root attributes are written automatically,
- how Python dicts are mapped to HDF5 groups and datasets,
- which attributes feed the content-addressable ID hash.

### The `ProductSchema` protocol

A schema is any class that satisfies the `ProductSchema` protocol defined in
`fd5._types`:

```python
class ProductSchema(Protocol):
    product_type: str          # e.g. "weather-station"
    schema_version: str        # semver string, e.g. "1.0.0"

    def json_schema(self) -> dict[str, Any]:
        """Return a JSON Schema dict that the sealed file must satisfy."""
        ...

    def required_root_attrs(self) -> dict[str, Any]:
        """Return attrs written to the HDF5 root group (e.g. product, domain)."""
        ...

    def write(self, target: Any, data: Any) -> None:
        """Write product-specific groups/datasets into *target*."""
        ...

    def id_inputs(self) -> list[str]:
        """Return root-attr keys whose values feed the deterministic ID hash."""
        ...
```

You do **not** need to inherit from a base class -- fd5 uses structural
(duck-type) checking via `typing.Protocol`.

### How fd5 discovers schemas

Schemas are discovered at runtime through Python **entry points** in the
`fd5.schemas` group. When you call `fd5.create(..., product="weather-station")`,
fd5 looks up the entry point named `weather-station`, instantiates the class,
and delegates writing to it.

For tests you can also register a schema directly with
`fd5.registry.register_schema()` without installing the package.

## 2. Worked Example: `weather-station` schema

Create a file `my_weather/schema.py`:

```python
# my_weather/schema.py
from __future__ import annotations

from typing import Any

import numpy as np


class WeatherStationSchema:
    """fd5 product schema for weather-station time series."""

    product_type: str = "weather-station"
    schema_version: str = "1.0.0"

    # ------------------------------------------------------------------ #
    # Protocol methods                                                     #
    # ------------------------------------------------------------------ #

    def json_schema(self) -> dict[str, Any]:
        return {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "measurements": {
                    "type": "object",
                    "properties": {
                        "temperature": {"type": "array"},
                        "humidity": {"type": "array"},
                        "pressure": {"type": "array"},
                        "wind_speed": {"type": "array"},
                    },
                    "required": ["temperature"],
                },
                "time": {"type": "array"},
                "station_id": {"type": "string"},
                "location": {"type": "object"},
            },
            "required": ["measurements", "time"],
        }

    def required_root_attrs(self) -> dict[str, Any]:
        return {"product": "weather-station", "domain": "environmental"}

    def id_inputs(self) -> list[str]:
        return ["product", "name", "timestamp", "station_id"]

    def write(self, target: Any, data: dict[str, Any]) -> None:
        """Write weather-station data into the HDF5 file.

        Parameters
        ----------
        target
            An h5py.Group-like object (may be a hash-tracking wrapper).
        data
            Dict with keys ``measurements``, ``time``, and optionally
            ``station_id`` and ``location``.
        """
        self._write_measurements(target, data["measurements"])
        self._write_time(target, data["time"])

        if "location" in data:
            self._write_location(target, data["location"])

        if "station_id" in data:
            target.attrs["station_id"] = data["station_id"]

    # ------------------------------------------------------------------ #
    # Private helpers                                                      #
    # ------------------------------------------------------------------ #

    def _write_measurements(self, target: Any, measurements: dict[str, Any]) -> None:
        units_map = {
            "temperature": "K",
            "humidity": "%",
            "pressure": "Pa",
            "wind_speed": "m/s",
        }
        grp = target.require_group("measurements")
        for name, values in measurements.items():
            arr = np.asarray(values, dtype=np.float64)
            ds = grp.create_dataset(name, data=arr, compression="gzip")
            ds.attrs["description"] = f"{name} measurements"
            if name in units_map:
                ds.attrs["units"] = units_map[name]

    def _write_time(self, target: Any, time_values: Any) -> None:
        arr = np.asarray(time_values, dtype=np.float64)
        ds = target.create_dataset("time", data=arr, compression="gzip")
        ds.attrs["units"] = "s"
        ds.attrs["description"] = "Elapsed time since measurement start"

    def _write_location(self, target: Any, location: dict[str, Any]) -> None:
        loc = target.require_group("location")
        for k, v in location.items():
            loc.attrs[k] = v
```

### Key design decisions

| Decision | Rationale |
|---|---|
| `json_schema()` requires `measurements` and `time` | Every weather file must have at least one measured variable and a time axis. |
| `id_inputs()` includes `station_id` | Two files from different stations at the same time get different IDs. |
| `_write_measurements` uses a units map | Keeps unit assignment DRY and easy to extend. |
| All datasets use `compression="gzip"` | Reduces file size for large time series. |
| Every dataset gets `@description` and `@units` | fd5 quality checks warn about missing descriptions. |

## 3. Register via entry points

In your package's `pyproject.toml`, add an entry point so fd5 can discover
the schema at runtime:

```toml
[project.entry-points."fd5.schemas"]
weather-station = "my_weather.schema:WeatherStationSchema"
```

The entry-point **name** (`weather-station`) must match the value you pass as
`product=` when creating a file. The entry-point **value** points to the
schema class. fd5 will call `WeatherStationSchema()` (no arguments) to
instantiate it.

After installing your package (`pip install -e .`), verify the schema is
visible:

```python
from fd5.registry import list_schemas
print(list_schemas())
# [..., 'weather-station']
```

## 4. Use it

```python
import fd5
import numpy as np

with fd5.create(
    "/tmp/weather",
    product="weather-station",
    name="zurich-central",
    description="Hourly weather measurements from Zurich central station",
    timestamp="2026-03-30T12:00:00Z",
) as b:
    # Set station_id as a root attribute so it is available to id_inputs()
    b.file.attrs["station_id"] = "ZRH-001"

    b.write_product({
        "measurements": {
            "temperature": np.array([293.15, 294.2, 295.0, 293.8]),
            "humidity": np.array([45.0, 42.0, 38.0, 50.0]),
            "pressure": np.array([101325.0, 101300.0, 101280.0, 101310.0]),
        },
        "time": np.array([0.0, 3600.0, 7200.0, 10800.0]),
        "station_id": "ZRH-001",
        "location": {
            "latitude": 47.3769,
            "longitude": 8.5417,
            "altitude_m": 408.0,
        },
    })
```

`fd5.create` writes the root attributes (`product`, `name`, `description`,
`timestamp`), then `b.write_product(...)` delegates to
`WeatherStationSchema.write()`. On context-manager exit the file is sealed:
the JSON schema is embedded, the content hash is computed, the deterministic
ID is derived from `id_inputs()`, and the file is atomically renamed to its
final content-addressed filename.

## 5. Verify

After the file is created you can verify its integrity and validate it
against the embedded schema:

```python
import fd5
import glob

# Find the sealed file (filename is generated from the ID hash)
path = glob.glob("/tmp/weather/*.h5")[0]

# Integrity check -- recomputes the Merkle-tree hash
assert fd5.verify(path)

# Schema validation -- checks file structure against the embedded JSON Schema
errors = fd5.validate(path)
assert errors == []
```

`fd5.verify()` returns `True` when the recomputed content hash matches the
stored one. `fd5.validate()` returns a list of `jsonschema.ValidationError`
objects -- an empty list means the file passes.

## 6. Tips and best practices

- **Use `compression="gzip"`** for every numeric dataset. It typically cuts
  file size in half with negligible write overhead.

- **Set `@units` on every numeric dataset.** This makes the file
  self-describing and enables automated unit conversion downstream.

- **Set `@description` on every dataset and group.** fd5 emits quality
  warnings during sealing for any dataset or group missing a description.

- **Keep `json_schema()` aligned with `write()`.** The schema is validated
  against the actual file contents at seal time. If `write()` produces
  groups/datasets that `json_schema()` does not expect (or vice versa), the
  file will fail to seal.

- **Use `id_inputs()` with domain-specific identifiers.** The ID hash is
  deterministic: the same inputs always produce the same ID. Include keys
  like `station_id`, `experiment_id`, or `sensor_serial` so that files from
  different sources get unique IDs.

- **Reference existing schemas for advanced patterns.** The built-in
  schemas in `src/fd5/imaging/` (e.g. `spectrum.py`, `recon.py`) show
  patterns for nested groups, fit results, multi-dimensional data, and
  metadata sub-groups.

## 7. Testing your schema

You can register a schema in tests without installing the package by using
`fd5.registry.register_schema()`:

```python
import fd5
import numpy as np
from fd5.registry import register_schema
from my_weather.schema import WeatherStationSchema


def test_weather_station_roundtrip(tmp_path):
    register_schema("weather-station", WeatherStationSchema())

    with fd5.create(
        tmp_path,
        product="weather-station",
        name="test-station",
        description="Test weather data",
        timestamp="2026-01-01T00:00:00Z",
    ) as b:
        b.file.attrs["station_id"] = "TEST-001"
        b.write_product({
            "measurements": {
                "temperature": np.array([290.0, 291.0]),
            },
            "time": np.array([0.0, 3600.0]),
            "station_id": "TEST-001",
        })

    path = next(tmp_path.glob("*.h5"))
    assert fd5.verify(str(path))

    errors = fd5.validate(str(path))
    assert errors == [], f"Validation errors: {errors}"
```

Run with pytest:

```bash
pytest tests/test_weather_schema.py -v
```

### What to test

- **Roundtrip:** create a file, verify it, validate it.
- **Minimal data:** only the required fields (e.g. `temperature` and `time`).
- **Full data:** all optional fields (`humidity`, `pressure`, `wind_speed`,
  `location`).
- **Missing required fields:** confirm that omitting `measurements` or `time`
  raises an error during sealing.
