"""Tests for fd5.generic.ndarray — NdarraySchema product schema."""

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
    from fd5.generic.ndarray import NdarraySchema

    return NdarraySchema()


@pytest.fixture()
def h5file(tmp_path):
    path = tmp_path / "ndarray.h5"
    with h5py.File(path, "w") as f:
        yield f


@pytest.fixture()
def h5path(tmp_path):
    return tmp_path / "ndarray.h5"


def _minimal_data():
    rng = np.random.default_rng(42)
    return {
        "array": rng.standard_normal((16, 32, 32)).astype(np.float32),
        "dimension_order": "ZYX",
        "description": "Test 3D array",
    }


def _data_with_affine():
    data = _minimal_data()
    data["affine"] = np.eye(4, dtype=np.float64)
    data["affine"][0, 3] = 10.0  # translation
    data["reference_frame"] = "LPS"
    return data


def _data_with_dimensions():
    data = _minimal_data()
    data["dimensions"] = {
        "Z": {"label": "slice", "units": "mm", "spacing": 2.0},
        "Y": {"label": "row", "units": "mm", "spacing": 1.0},
        "X": {"label": "column", "units": "mm", "spacing": 1.0},
    }
    return data


def _2d_data():
    rng = np.random.default_rng(99)
    return {
        "array": rng.standard_normal((64, 64)).astype(np.float64),
        "dimension_order": "YX",
        "description": "Test 2D array",
    }


def _integer_data():
    return {
        "array": np.arange(24, dtype=np.int32).reshape(2, 3, 4),
        "dimension_order": "CYX",
        "description": "Integer 3D array",
    }


# ---------------------------------------------------------------------------
# Protocol conformance
# ---------------------------------------------------------------------------


class TestProtocolConformance:
    def test_satisfies_product_schema_protocol(self, schema):
        assert isinstance(schema, ProductSchema)

    def test_product_type_is_ndarray(self, schema):
        assert schema.product_type == "ndarray"

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

    def test_product_const_is_ndarray(self, schema):
        result = schema.json_schema()
        assert result["properties"]["product"]["const"] == "ndarray"

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

    def test_contains_product_ndarray(self, schema):
        result = schema.required_root_attrs()
        assert result["product"] == "ndarray"


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
    def test_writes_array_dataset(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "array" in h5file

    def test_array_shape(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["array"].shape == (16, 32, 32)

    def test_array_dtype(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["array"].dtype == np.float32

    def test_array_gzip_compressed(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file["array"].compression == "gzip"
        assert h5file["array"].compression_opts == 4

    def test_dimension_order_attr(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert h5file.attrs["dimension_order"] == "ZYX"

    def test_no_reference_frame_when_absent(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "reference_frame" not in h5file.attrs

    def test_no_affine_when_absent(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "affine" not in h5file

    def test_no_dimensions_when_absent(self, schema, h5file):
        schema.write(h5file, _minimal_data())
        assert "dimensions" not in h5file

    def test_roundtrip_array_data(self, schema, h5file):
        data = _minimal_data()
        schema.write(h5file, data)
        np.testing.assert_array_almost_equal(h5file["array"][:], data["array"])


# ---------------------------------------------------------------------------
# write() — with affine and reference_frame
# ---------------------------------------------------------------------------


class TestWriteAffine:
    def test_affine_dataset_created(self, schema, h5file):
        schema.write(h5file, _data_with_affine())
        assert "affine" in h5file

    def test_affine_shape(self, schema, h5file):
        schema.write(h5file, _data_with_affine())
        assert h5file["affine"].shape == (4, 4)

    def test_affine_dtype(self, schema, h5file):
        schema.write(h5file, _data_with_affine())
        assert h5file["affine"].dtype == np.float64

    def test_affine_translation(self, schema, h5file):
        schema.write(h5file, _data_with_affine())
        assert h5file["affine"][0, 3] == pytest.approx(10.0)

    def test_reference_frame_attr(self, schema, h5file):
        schema.write(h5file, _data_with_affine())
        assert h5file.attrs["reference_frame"] == "LPS"


# ---------------------------------------------------------------------------
# write() — with dimensions
# ---------------------------------------------------------------------------


class TestWriteDimensions:
    def test_dimensions_group_created(self, schema, h5file):
        schema.write(h5file, _data_with_dimensions())
        assert "dimensions" in h5file

    def test_dimension_axis_groups(self, schema, h5file):
        schema.write(h5file, _data_with_dimensions())
        assert "dimensions/Z" in h5file
        assert "dimensions/Y" in h5file
        assert "dimensions/X" in h5file

    def test_dimension_attrs(self, schema, h5file):
        schema.write(h5file, _data_with_dimensions())
        z = h5file["dimensions/Z"]
        assert z.attrs["label"] == "slice"
        assert z.attrs["units"] == "mm"
        assert z.attrs["spacing"] == pytest.approx(2.0)

    def test_all_dims_have_attrs(self, schema, h5file):
        schema.write(h5file, _data_with_dimensions())
        for axis in ("Z", "Y", "X"):
            grp = h5file[f"dimensions/{axis}"]
            assert "label" in grp.attrs
            assert "units" in grp.attrs
            assert "spacing" in grp.attrs


# ---------------------------------------------------------------------------
# write() — 2D and integer data
# ---------------------------------------------------------------------------


class TestWriteVariousShapes:
    def test_2d_array(self, schema, h5file):
        schema.write(h5file, _2d_data())
        assert h5file["array"].shape == (64, 64)
        assert h5file.attrs["dimension_order"] == "YX"

    def test_integer_array(self, schema, h5file):
        schema.write(h5file, _integer_data())
        assert h5file["array"].dtype == np.int32
        assert h5file["array"].shape == (2, 3, 4)


# ---------------------------------------------------------------------------
# Integration — round-trip
# ---------------------------------------------------------------------------


class TestIntegration:
    def test_create_verify_roundtrip(self, schema, tmp_path):
        import fd5

        register_schema("ndarray", schema)
        data = _minimal_data()
        with fd5.create(
            tmp_path,
            product="ndarray",
            name="roundtrip-nda",
            description="Ndarray round-trip test",
            timestamp="2026-01-01T00:00:00Z",
        ) as b:
            b.write_product(data)

        files = list(tmp_path.glob("*.h5"))
        assert len(files) == 1
        assert fd5.verify(str(files[0]))

    def test_create_validate_roundtrip(self, schema, h5path):
        from fd5.schema import embed_schema, validate

        register_schema("ndarray", schema)
        data = _data_with_affine()
        with h5py.File(h5path, "w") as f:
            for k, v in schema.required_root_attrs().items():
                f.attrs[k] = v
            f.attrs["name"] = "integration-test-ndarray"
            f.attrs["description"] = "Integration test ndarray"
            embed_schema(f, schema.json_schema())
            schema.write(f, data)

        errors = validate(h5path)
        assert errors == [], [e.message for e in errors]

    def test_roundtrip_with_dimensions(self, schema, tmp_path):
        import fd5

        register_schema("ndarray", schema)
        data = _data_with_dimensions()
        with fd5.create(
            tmp_path,
            product="ndarray",
            name="dims-nda",
            description="Ndarray with dimensions",
            timestamp="2026-01-01T00:00:00Z",
        ) as b:
            b.write_product(data)

        files = list(tmp_path.glob("*.h5"))
        assert len(files) == 1
        assert fd5.verify(str(files[0]))
