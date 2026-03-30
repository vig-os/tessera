"""Tests for fd5.generic.timeseries — TimeseriesSchema product schema."""

from __future__ import annotations

import h5py
import numpy as np
import pytest

from fd5.registry import ProductSchema, register_schema


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture()
def schema():
    from fd5.generic.timeseries import TimeseriesSchema

    return TimeseriesSchema()


@pytest.fixture()
def h5file(tmp_path):
    path = tmp_path / "timeseries.h5"
    with h5py.File(path, "w") as f:
        yield f


@pytest.fixture()
def h5path(tmp_path):
    return tmp_path / "timeseries.h5"


def _minimal_data():
    rng = np.random.default_rng(42)
    return {
        "signals": {
            "ecg": rng.standard_normal(1000).astype(np.float64),
            "resp": rng.standard_normal(1000).astype(np.float64),
        },
        "time": np.linspace(0, 10, 1000),
        "sampling_rate": 100.0,
        "description": "Test timeseries",
    }


def _data_with_events():
    data = _minimal_data()
    data["events"] = {
        "timestamps": np.array([1.0, 3.5, 7.2]),
        "labels": ["start", "peak", "end"],
    }
    return data


def _data_with_metadata():
    data = _minimal_data()
    data["metadata"] = {
        "sensor_type": "optical",
        "gain": 2.5,
        "channel_count": 2,
    }
    return data


# ---------------------------------------------------------------------------
# Protocol conformance
# ---------------------------------------------------------------------------


class TestProtocolConformance:
    def test_satisfies_product_schema_protocol(self, schema):
        assert isinstance(schema, ProductSchema)

    def test_product_type_is_timeseries(self, schema):
        assert schema.product_type == "timeseries"

    def test_schema_version_is_string(self, schema):
        assert isinstance(schema.schema_version, str)

    def test_has_required_methods(self, schema):
        assert callable(schema.json_schema)
        assert callable(schema.required_root_attrs)
        assert callable(schema.write)
        assert callable(schema.id_inputs)


# ---------------------------------------------------------------------------
# json_schema()
# ---------------------------------------------------------------------------


class TestJsonSchema:
    def test_returns_dict(self, schema):
        result = schema.json_schema()
        assert isinstance(result, dict)

    def test_has_draft_2020_12_meta(self, schema):
        result = schema.json_schema()
        assert result["$schema"] == "https://json-schema.org/draft/2020-12/schema"

    def test_product_const_is_timeseries(self, schema):
        result = schema.json_schema()
        assert result["properties"]["product"]["const"] == "timeseries"

    def test_valid_json_schema(self, schema):
        import jsonschema

        result = schema.json_schema()
        jsonschema.Draft202012Validator.check_schema(result)


# ---------------------------------------------------------------------------
# required_root_attrs()
# ---------------------------------------------------------------------------


class TestRequiredRootAttrs:
    def test_returns_dict(self, schema):
        result = schema.required_root_attrs()
        assert isinstance(result, dict)

    def test_contains_product_timeseries(self, schema):
        result = schema.required_root_attrs()
        assert result["product"] == "timeseries"


# ---------------------------------------------------------------------------
# id_inputs()
# ---------------------------------------------------------------------------


class TestIdInputs:
    def test_returns_list_of_strings(self, schema):
        result = schema.id_inputs()
        assert isinstance(result, list)
        assert all(isinstance(s, str) for s in result)

    def test_contains_expected_keys(self, schema):
        result = schema.id_inputs()
        assert "product" in result
        assert "name" in result
        assert "timestamp" in result

    def test_returns_fresh_list(self, schema):
        a = schema.id_inputs()
        b = schema.id_inputs()
        assert a is not b

    def test_deterministic(self, schema):
        assert schema.id_inputs() == schema.id_inputs()


# ---------------------------------------------------------------------------
# write() — minimal data
# ---------------------------------------------------------------------------


class TestWriteMinimal:
    def test_writes_signals_group(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "signals" in h5file

    def test_writes_signal_channels(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "signals/ecg" in h5file
        assert "signals/resp" in h5file

    def test_signal_dtype(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["signals/ecg"].dtype == np.float64

    def test_signal_shape(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["signals/ecg"].shape == (1000,)

    def test_signal_gzip_compressed(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["signals/ecg"].compression == "gzip"
        assert h5file["signals/ecg"].compression_opts == 4

    def test_signal_has_units_attr(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "units" in h5file["signals/ecg"].attrs

    def test_signal_has_description_attr(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "description" in h5file["signals/ecg"].attrs

    def test_writes_time_dataset(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "time" in h5file
        assert h5file["time"].dtype == np.float64

    def test_time_units_attr(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["time"].attrs["units"] == "s"

    def test_time_shape(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["time"].shape == (1000,)

    def test_sampling_rate_attr(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file.attrs["sampling_rate"] == pytest.approx(100.0)

    def test_sampling_rate_units_attr(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file.attrs["sampling_rate_units"] == "Hz"

    def test_roundtrip_time_data(self, schema, h5file):
        data = _minimal_data()
        schema.write(h5file, data)
        np.testing.assert_array_almost_equal(h5file["time"][:], data["time"])

    def test_no_events_when_absent(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "events" not in h5file


# ---------------------------------------------------------------------------
# write() — with events
# ---------------------------------------------------------------------------


class TestWriteEvents:
    def test_events_group_created(self, schema, h5file):
        schema.write(h5file, _data_with_events())
        assert "events" in h5file

    def test_events_timestamps(self, schema, h5file):
        data = _data_with_events()
        schema.write(h5file, data)
        np.testing.assert_array_almost_equal(
            h5file["events/timestamps"][:], [1.0, 3.5, 7.2]
        )

    def test_events_labels(self, schema, h5file):
        schema.write(h5file, _data_with_events())
        raw = h5file["events/labels"][:]
        labels = [v.decode("utf-8") if isinstance(v, bytes) else str(v) for v in raw]
        assert labels == ["start", "peak", "end"]


# ---------------------------------------------------------------------------
# write() — with metadata
# ---------------------------------------------------------------------------


class TestWriteMetadata:
    def test_metadata_attrs_written(self, schema, h5file):
        schema.write(h5file, _data_with_metadata())
        assert h5file.attrs["sensor_type"] == "optical"
        assert float(h5file.attrs["gain"]) == pytest.approx(2.5)
        assert int(h5file.attrs["channel_count"]) == 2


# ---------------------------------------------------------------------------
# write() — single channel
# ---------------------------------------------------------------------------


class TestWriteSingleChannel:
    def test_single_channel(self, schema, h5file):
        data = {
            "signals": {"temperature": np.ones(100)},
            "time": np.arange(100, dtype=np.float64),
            "description": "Single channel",
        }
        schema.write(h5file, data)
        assert "signals/temperature" in h5file
        assert h5file["signals/temperature"].shape == (100,)


# ---------------------------------------------------------------------------
# Integration — round-trip
# ---------------------------------------------------------------------------


class TestIntegration:
    def test_create_verify_roundtrip(self, schema, tmp_path):
        import fd5

        register_schema("timeseries", schema)
        data = _minimal_data()
        with fd5.create(
            tmp_path,
            product="timeseries",
            name="roundtrip-ts",
            description="Timeseries round-trip test",
            timestamp="2026-01-01T00:00:00Z",
        ) as b:
            b.write_product(data)

        files = list(tmp_path.glob("*.h5"))
        assert len(files) == 1
        assert fd5.verify(str(files[0]))

    def test_create_validate_roundtrip(self, schema, h5path):
        from fd5.schema import embed_schema, validate

        register_schema("timeseries", schema)
        data = _minimal_data()
        with h5py.File(h5path, "w") as f:
            for k, v in schema.required_root_attrs().items():
                f.attrs[k] = v
            f.attrs["name"] = "integration-test-timeseries"
            f.attrs["description"] = "Integration test timeseries"
            embed_schema(f, schema.json_schema())
            schema.write(f, data)

        errors = validate(h5path)
        assert errors == [], [e.message for e in errors]

    def test_roundtrip_with_events(self, schema, tmp_path):
        import fd5

        register_schema("timeseries", schema)
        data = _data_with_events()
        with fd5.create(
            tmp_path,
            product="timeseries",
            name="events-ts",
            description="Timeseries with events",
            timestamp="2026-01-01T00:00:00Z",
        ) as b:
            b.write_product(data)

        files = list(tmp_path.glob("*.h5"))
        assert len(files) == 1
        assert fd5.verify(str(files[0]))
