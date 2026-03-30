"""Tests for fd5.generic.tabular — TabularSchema product schema."""

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
    from fd5.generic.tabular import TabularSchema

    return TabularSchema()


@pytest.fixture()
def h5file(tmp_path):
    path = tmp_path / "tabular.h5"
    with h5py.File(path, "w") as f:
        yield f


@pytest.fixture()
def h5path(tmp_path):
    return tmp_path / "tabular.h5"


def _minimal_data():
    rng = np.random.default_rng(42)
    return {
        "columns": {
            "temperature": rng.uniform(20, 30, size=50).astype(np.float64),
            "pressure": rng.uniform(990, 1020, size=50).astype(np.float64),
            "humidity": rng.uniform(30, 80, size=50).astype(np.float64),
        },
        "description": "Test tabular data",
    }


def _data_with_column_metadata():
    data = _minimal_data()
    data["column_metadata"] = {
        "temperature": {"units": "C", "description": "Ambient temperature"},
        "pressure": {"units": "hPa", "description": "Atmospheric pressure"},
        "humidity": {"units": "%", "description": "Relative humidity"},
    }
    return data


def _data_with_row_labels():
    data = _minimal_data()
    data["row_labels"] = [f"sample_{i}" for i in range(50)]
    return data


def _integer_data():
    return {
        "columns": {
            "count": np.arange(20, dtype=np.int64),
            "flag": np.ones(20, dtype=np.int32),
        },
        "description": "Integer tabular data",
    }


# ---------------------------------------------------------------------------
# Protocol conformance
# ---------------------------------------------------------------------------


class TestProtocolConformance:
    def test_satisfies_product_schema_protocol(self, schema):
        assert isinstance(schema, ProductSchema)

    def test_product_type_is_tabular(self, schema):
        assert schema.product_type == "tabular"

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

    def test_product_const_is_tabular(self, schema):
        result = schema.json_schema()
        assert result["properties"]["product"]["const"] == "tabular"

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

    def test_contains_product_tabular(self, schema):
        result = schema.required_root_attrs()
        assert result["product"] == "tabular"


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
    def test_writes_table_group(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "table" in h5file

    def test_writes_columns(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "table/temperature" in h5file
        assert "table/pressure" in h5file
        assert "table/humidity" in h5file

    def test_column_shape(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["table/temperature"].shape == (50,)

    def test_column_gzip_compressed(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["table/temperature"].compression == "gzip"
        assert h5file["table/temperature"].compression_opts == 4

    def test_column_count_attr(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert int(h5file.attrs["column_count"]) == 3

    def test_row_count_attr(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert int(h5file.attrs["row_count"]) == 50

    def test_no_row_labels_when_absent(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "row_labels" not in h5file

    def test_roundtrip_column_data(self, schema, h5file):
        data = _minimal_data()
        schema.write(h5file, data)
        np.testing.assert_array_almost_equal(
            h5file["table/temperature"][:], data["columns"]["temperature"]
        )


# ---------------------------------------------------------------------------
# write() — with column metadata
# ---------------------------------------------------------------------------


class TestWriteColumnMetadata:
    def test_column_units_attr(self, schema, h5file):
        schema.write(h5file, _data_with_column_metadata())
        assert h5file["table/temperature"].attrs["units"] == "C"

    def test_column_description_attr(self, schema, h5file):
        schema.write(h5file, _data_with_column_metadata())
        assert h5file["table/temperature"].attrs["description"] == "Ambient temperature"

    def test_all_columns_have_metadata(self, schema, h5file):
        schema.write(h5file, _data_with_column_metadata())
        for col in ("temperature", "pressure", "humidity"):
            assert "units" in h5file[f"table/{col}"].attrs
            assert "description" in h5file[f"table/{col}"].attrs


# ---------------------------------------------------------------------------
# write() — with row labels
# ---------------------------------------------------------------------------


class TestWriteRowLabels:
    def test_row_labels_created(self, schema, h5file):
        schema.write(h5file, _data_with_row_labels())
        assert "row_labels" in h5file

    def test_row_labels_values(self, schema, h5file):
        schema.write(h5file, _data_with_row_labels())
        raw = h5file["row_labels"][:]
        labels = [v.decode("utf-8") if isinstance(v, bytes) else str(v) for v in raw]
        assert labels[0] == "sample_0"
        assert labels[-1] == "sample_49"
        assert len(labels) == 50


# ---------------------------------------------------------------------------
# write() — integer data
# ---------------------------------------------------------------------------


class TestWriteIntegerData:
    def test_integer_columns(self, schema, h5file):
        schema.write(h5file, _integer_data())
        assert h5file["table/count"].dtype == np.int64
        assert h5file["table/flag"].dtype == np.int32

    def test_integer_row_count(self, schema, h5file):
        schema.write(h5file, _integer_data())
        assert int(h5file.attrs["row_count"]) == 20


# ---------------------------------------------------------------------------
# Integration — round-trip
# ---------------------------------------------------------------------------


class TestIntegration:
    def test_create_verify_roundtrip(self, schema, tmp_path):
        import fd5

        register_schema("tabular", schema)
        data = _minimal_data()
        with fd5.create(
            tmp_path,
            product="tabular",
            name="roundtrip-tab",
            description="Tabular round-trip test",
            timestamp="2026-01-01T00:00:00Z",
        ) as b:
            b.write_product(data)

        files = list(tmp_path.glob("*.h5"))
        assert len(files) == 1
        assert fd5.verify(str(files[0]))

    def test_create_validate_roundtrip(self, schema, h5path):
        from fd5.schema import embed_schema, validate

        register_schema("tabular", schema)
        data = _data_with_column_metadata()
        with h5py.File(h5path, "w") as f:
            for k, v in schema.required_root_attrs().items():
                f.attrs[k] = v
            f.attrs["name"] = "integration-test-tabular"
            f.attrs["description"] = "Integration test tabular"
            embed_schema(f, schema.json_schema())
            schema.write(f, data)

        errors = validate(h5path)
        assert errors == [], [e.message for e in errors]

    def test_roundtrip_with_row_labels(self, schema, tmp_path):
        import fd5

        register_schema("tabular", schema)
        data = _data_with_row_labels()
        with fd5.create(
            tmp_path,
            product="tabular",
            name="labels-tab",
            description="Tabular with row labels",
            timestamp="2026-01-01T00:00:00Z",
        ) as b:
            b.write_product(data)

        files = list(tmp_path.glob("*.h5"))
        assert len(files) == 1
        assert fd5.verify(str(files[0]))
