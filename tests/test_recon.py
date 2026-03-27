"""Tests for fd5.imaging.recon — ReconSchema product schema."""

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
    from fd5.imaging.recon import ReconSchema

    return ReconSchema()


@pytest.fixture()
def h5file(tmp_path):
    path = tmp_path / "recon.h5"
    with h5py.File(path, "w") as f:
        yield f


@pytest.fixture()
def h5path(tmp_path):
    return tmp_path / "recon.h5"


def _make_volume_3d(shape=(64, 128, 128)):
    return np.random.default_rng(42).random(shape, dtype=np.float32)


def _make_volume_4d(shape=(4, 32, 64, 64)):
    return np.random.default_rng(42).random(shape, dtype=np.float32)


def _make_affine():
    aff = np.eye(4, dtype=np.float64)
    aff[0, 0] = 2.0  # 2mm voxel spacing Z
    aff[1, 1] = 1.0
    aff[2, 2] = 1.0
    return aff


def _minimal_3d_data():
    vol = _make_volume_3d((16, 32, 32))
    return {
        "volume": vol,
        "affine": _make_affine(),
        "dimension_order": "ZYX",
        "reference_frame": "LPS",
        "description": "Test static CT reconstruction volume",
    }


def _minimal_4d_data():
    vol = _make_volume_4d((3, 8, 16, 16))
    return {
        "volume": vol,
        "affine": _make_affine(),
        "dimension_order": "TZYX",
        "reference_frame": "LPS",
        "description": "Test dynamic PET reconstruction volume",
        "frames": {
            "n_frames": 3,
            "frame_type": "time",
            "description": "Dynamic time frames for PET reconstruction",
            "frame_start": np.array([0.0, 60.0, 120.0]),
            "frame_duration": np.array([60.0, 60.0, 60.0]),
            "frame_label": ["frame_0", "frame_1", "frame_2"],
        },
    }


# ---------------------------------------------------------------------------
# Protocol conformance
# ---------------------------------------------------------------------------


class TestProtocolConformance:
    def test_satisfies_product_schema_protocol(self, schema):
        assert isinstance(schema, ProductSchema)

    def test_product_type_is_recon(self, schema):
        assert schema.product_type == "recon"

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

    def test_product_const_is_recon(self, schema):
        result = schema.json_schema()
        assert result["properties"]["product"]["const"] == "recon"

    def test_requires_volume(self, schema):
        result = schema.json_schema()
        assert "volume" in result.get("required", []) or "volume" in result.get(
            "properties", {}
        )

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

    def test_contains_product_recon(self, schema):
        result = schema.required_root_attrs()
        assert result["product"] == "recon"

    def test_contains_domain(self, schema):
        result = schema.required_root_attrs()
        assert result["domain"] == "medical_imaging"


# ---------------------------------------------------------------------------
# id_inputs()
# ---------------------------------------------------------------------------


class TestIdInputs:
    def test_returns_list_of_strings(self, schema):
        result = schema.id_inputs()
        assert isinstance(result, list)
        assert all(isinstance(s, str) for s in result)

    def test_follows_medical_imaging_convention(self, schema):
        result = schema.id_inputs()
        assert "timestamp" in result
        assert "scanner" in result
        assert "vendor_series_id" in result


# ---------------------------------------------------------------------------
# write() — 3D static volume
# ---------------------------------------------------------------------------


class TestWrite3D:
    def test_writes_volume_dataset(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "volume" in h5file
        assert h5file["volume"].dtype == np.float32

    def test_volume_shape_matches(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert h5file["volume"].shape == (16, 32, 32)

    def test_volume_has_affine_attr(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        aff = h5file["volume"].attrs["affine"]
        assert aff.shape == (4, 4)
        assert aff.dtype == np.float64

    def test_volume_has_dimension_order(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert h5file["volume"].attrs["dimension_order"] == "ZYX"

    def test_volume_has_reference_frame(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert h5file["volume"].attrs["reference_frame"] == "LPS"

    def test_volume_has_description(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "description" in h5file["volume"].attrs

    def test_3d_chunking_strategy(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        chunks = h5file["volume"].chunks
        assert chunks == (1, 32, 32)

    def test_gzip_compression_level_4(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert h5file["volume"].compression == "gzip"
        assert h5file["volume"].compression_opts == 4

    def test_no_frames_group_for_3d(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "frames" not in h5file


# ---------------------------------------------------------------------------
# write() — 4D dynamic volume
# ---------------------------------------------------------------------------


class TestWrite4D:
    def test_writes_volume_dataset(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        assert "volume" in h5file
        assert h5file["volume"].shape == (3, 8, 16, 16)

    def test_4d_chunking_strategy(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        chunks = h5file["volume"].chunks
        assert chunks == (1, 1, 16, 16)

    def test_4d_dimension_order(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        assert h5file["volume"].attrs["dimension_order"] == "TZYX"

    def test_frames_group_exists(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        assert "frames" in h5file
        assert isinstance(h5file["frames"], h5py.Group)

    def test_frames_attrs(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        grp = h5file["frames"]
        assert grp.attrs["n_frames"] == 3
        assert grp.attrs["frame_type"] == "time"
        assert "description" in grp.attrs

    def test_frame_start_dataset(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        ds = h5file["frames/frame_start"]
        np.testing.assert_array_almost_equal(ds[:], [0.0, 60.0, 120.0])

    def test_frame_duration_dataset(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        ds = h5file["frames/frame_duration"]
        np.testing.assert_array_almost_equal(ds[:], [60.0, 60.0, 60.0])

    def test_frame_label_dataset(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        ds = h5file["frames/frame_label"]
        labels = [v.decode() if isinstance(v, bytes) else str(v) for v in ds[:]]
        assert labels == ["frame_0", "frame_1", "frame_2"]


# ---------------------------------------------------------------------------
# write() — pyramid
# ---------------------------------------------------------------------------


class TestWritePyramid:
    def test_pyramid_group_created(self, schema, h5file):
        data = _minimal_3d_data()
        data["pyramid"] = {
            "scale_factors": [2, 4],
            "method": "local_mean",
        }
        schema.write(h5file, data)
        assert "pyramid" in h5file

    def test_pyramid_attrs(self, schema, h5file):
        data = _minimal_3d_data()
        data["pyramid"] = {
            "scale_factors": [2, 4],
            "method": "local_mean",
        }
        schema.write(h5file, data)
        grp = h5file["pyramid"]
        assert grp.attrs["n_levels"] == 2
        np.testing.assert_array_equal(grp.attrs["scale_factors"], [2, 4])
        assert grp.attrs["method"] == "local_mean"

    def test_pyramid_levels_created(self, schema, h5file):
        data = _minimal_3d_data()
        data["pyramid"] = {
            "scale_factors": [2, 4],
            "method": "local_mean",
        }
        schema.write(h5file, data)
        assert "pyramid/level_1" in h5file
        assert "pyramid/level_1/volume" in h5file
        assert "pyramid/level_2" in h5file
        assert "pyramid/level_2/volume" in h5file

    def test_pyramid_level_has_scale_factor_attr(self, schema, h5file):
        data = _minimal_3d_data()
        data["pyramid"] = {
            "scale_factors": [2],
            "method": "local_mean",
        }
        schema.write(h5file, data)
        ds = h5file["pyramid/level_1/volume"]
        assert ds.attrs["scale_factor"] == 2

    def test_pyramid_level_has_affine(self, schema, h5file):
        data = _minimal_3d_data()
        data["pyramid"] = {
            "scale_factors": [2],
            "method": "local_mean",
        }
        schema.write(h5file, data)
        ds = h5file["pyramid/level_1/volume"]
        assert "affine" in ds.attrs
        assert ds.attrs["affine"].shape == (4, 4)

    def test_pyramid_level_shape_downsampled(self, schema, h5file):
        data = _minimal_3d_data()
        data["pyramid"] = {
            "scale_factors": [2],
            "method": "local_mean",
        }
        schema.write(h5file, data)
        ds = h5file["pyramid/level_1/volume"]
        assert ds.shape == (8, 16, 16)

    def test_pyramid_level_compression(self, schema, h5file):
        data = _minimal_3d_data()
        data["pyramid"] = {
            "scale_factors": [2],
            "method": "local_mean",
        }
        schema.write(h5file, data)
        ds = h5file["pyramid/level_1/volume"]
        assert ds.compression == "gzip"
        assert ds.compression_opts == 4


# ---------------------------------------------------------------------------
# write() — MIP projections
# ---------------------------------------------------------------------------


class TestWriteMIP:
    def test_mips_group_created(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "mips" in h5file
        assert isinstance(h5file["mips"], h5py.Group)

    def test_mip_coronal_created(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "mips/coronal" in h5file

    def test_mip_sagittal_created(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "mips/sagittal" in h5file

    def test_mip_axial_created(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "mips/axial" in h5file

    def test_mip_coronal_shape_3d(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        ds = h5file["mips/coronal"]
        # 3D (16, 32, 32) → coronal collapses Y (axis 1) → (16, 32)
        assert ds.shape == (16, 32)

    def test_mip_sagittal_shape_3d(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        ds = h5file["mips/sagittal"]
        # 3D (16, 32, 32) → sagittal collapses X (axis 2) → (16, 32)
        assert ds.shape == (16, 32)

    def test_mip_axial_shape_3d(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        ds = h5file["mips/axial"]
        # 3D (16, 32, 32) → axial collapses Z (axis 0) → (32, 32)
        assert ds.shape == (32, 32)

    def test_mip_coronal_attrs(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        ds = h5file["mips/coronal"]
        assert ds.attrs["projection_type"] == "mip"
        assert "description" in ds.attrs

    def test_mip_sagittal_attrs(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        ds = h5file["mips/sagittal"]
        assert ds.attrs["projection_type"] == "mip"
        assert "description" in ds.attrs

    def test_mip_axial_attrs(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        ds = h5file["mips/axial"]
        assert ds.attrs["projection_type"] == "mip"
        assert "description" in ds.attrs

    def test_mip_dtype_float32(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert h5file["mips/coronal"].dtype == np.float32
        assert h5file["mips/sagittal"].dtype == np.float32
        assert h5file["mips/axial"].dtype == np.float32

    def test_mip_3d_coronal_values(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        vol = data["volume"]
        expected = vol.max(axis=1).astype(np.float32)
        np.testing.assert_array_almost_equal(h5file["mips/coronal"][:], expected)

    def test_mip_3d_axial_values(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        vol = data["volume"]
        expected = vol.max(axis=0).astype(np.float32)
        np.testing.assert_array_almost_equal(h5file["mips/axial"][:], expected)


# ---------------------------------------------------------------------------
# write() — MIP N-D arrays for 4D+ data
# ---------------------------------------------------------------------------


class TestWriteMIP4D:
    def test_mip_4d_coronal_shape(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        ds = h5file["mips/coronal"]
        n_frames, z, y, x = data["volume"].shape
        # 4D: coronal collapses Y (axis 2) → (T, Z, X)
        assert ds.shape == (n_frames, z, x)
        assert ds.dtype == np.float32

    def test_mip_4d_sagittal_shape(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        ds = h5file["mips/sagittal"]
        n_frames, z, y, x = data["volume"].shape
        # 4D: sagittal collapses X (axis 3) → (T, Z, Y)
        assert ds.shape == (n_frames, z, y)
        assert ds.dtype == np.float32

    def test_mip_4d_axial_shape(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        ds = h5file["mips/axial"]
        n_frames, z, y, x = data["volume"].shape
        # 4D: axial collapses Z (axis 1) → (T, Y, X)
        assert ds.shape == (n_frames, y, x)
        assert ds.dtype == np.float32

    def test_mip_4d_coronal_values(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        vol = data["volume"]
        # Coronal = max along Y (axis 2)
        expected = vol.max(axis=2).astype(np.float32)
        np.testing.assert_array_almost_equal(h5file["mips/coronal"][:], expected)

    def test_mip_4d_per_frame_coronal_matches(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        vol = data["volume"]
        # Frame 0 coronal should match vol[0].max(axis=1)
        expected_cor_0 = vol[0].max(axis=1).astype(np.float32)
        np.testing.assert_array_almost_equal(h5file["mips/coronal"][0], expected_cor_0)

    def test_mip_4d_axial_values(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        vol = data["volume"]
        expected = vol.max(axis=1).astype(np.float32)
        np.testing.assert_array_almost_equal(h5file["mips/axial"][:], expected)


# ---------------------------------------------------------------------------
# write() — gate_phase and gate_trigger (optional gated recon)
# ---------------------------------------------------------------------------


def _gated_cardiac_data():
    vol = _make_volume_4d((8, 8, 16, 16))
    return {
        "volume": vol,
        "affine": _make_affine(),
        "dimension_order": "TZYX",
        "reference_frame": "LPS",
        "description": "Gated cardiac reconstruction",
        "frames": {
            "n_frames": 8,
            "frame_type": "gate_cardiac",
            "description": "Cardiac gated frames",
            "frame_start": np.linspace(0.0, 0.7, 8),
            "frame_duration": np.full(8, 0.1),
            "gate_phase": np.linspace(0.0, 87.5, 8),
            "gate_trigger": {
                "signal": np.sin(np.linspace(0, 4 * np.pi, 500)),
                "sampling_rate": 500.0,
                "trigger_times": np.array([0.0, 0.8, 1.6]),
            },
        },
    }


class TestWriteGating:
    def test_gate_phase_dataset_created(self, schema, h5file):
        data = _gated_cardiac_data()
        schema.write(h5file, data)
        assert "frames/gate_phase" in h5file

    def test_gate_phase_values(self, schema, h5file):
        data = _gated_cardiac_data()
        schema.write(h5file, data)
        ds = h5file["frames/gate_phase"]
        np.testing.assert_array_almost_equal(ds[:], data["frames"]["gate_phase"])

    def test_gate_phase_attrs(self, schema, h5file):
        data = _gated_cardiac_data()
        schema.write(h5file, data)
        ds = h5file["frames/gate_phase"]
        assert ds.attrs["units"] == "%"
        assert "description" in ds.attrs

    def test_gate_trigger_group_created(self, schema, h5file):
        data = _gated_cardiac_data()
        schema.write(h5file, data)
        assert "frames/gate_trigger" in h5file
        assert isinstance(h5file["frames/gate_trigger"], h5py.Group)

    def test_gate_trigger_signal(self, schema, h5file):
        data = _gated_cardiac_data()
        schema.write(h5file, data)
        ds = h5file["frames/gate_trigger/signal"]
        np.testing.assert_array_almost_equal(
            ds[:], data["frames"]["gate_trigger"]["signal"]
        )
        assert ds.attrs["description"] == "Raw physiological gating signal"

    def test_gate_trigger_sampling_rate(self, schema, h5file):
        data = _gated_cardiac_data()
        schema.write(h5file, data)
        sr = h5file["frames/gate_trigger/sampling_rate"]
        assert sr.attrs["value"] == pytest.approx(500.0)
        assert sr.attrs["units"] == "Hz"
        assert sr.attrs["unitSI"] == pytest.approx(1.0)

    def test_gate_trigger_times(self, schema, h5file):
        data = _gated_cardiac_data()
        schema.write(h5file, data)
        ds = h5file["frames/gate_trigger/trigger_times"]
        np.testing.assert_array_almost_equal(ds[:], [0.0, 0.8, 1.6])
        assert ds.attrs["units"] == "s"

    def test_no_gate_phase_for_time_frames(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        assert "frames/gate_phase" not in h5file

    def test_no_gate_trigger_for_time_frames(self, schema, h5file):
        data = _minimal_4d_data()
        schema.write(h5file, data)
        assert "frames/gate_trigger" not in h5file


# ---------------------------------------------------------------------------
# write() — device_data (optional embedded device signals)
# ---------------------------------------------------------------------------


def _ecg_channel():
    n = 500
    return {
        "_type": "ecg",
        "_version": 1,
        "model": "GE CardioLab",
        "measurement": "voltage",
        "run_control": True,
        "description": "ECG trace for cardiac gating",
        "sampling_rate": 500.0,
        "signal": np.sin(np.linspace(0, 4 * np.pi, n)),
        "time": np.linspace(0.0, 1.0, n),
        "units": "mV",
        "unitSI": 0.001,
    }


def _bellows_channel():
    n = 200
    return {
        "_type": "bellows",
        "description": "Respiratory bellows signal",
        "sampling_rate": 50.0,
        "signal": np.sin(np.linspace(0, 2 * np.pi, n)),
        "time": np.linspace(0.0, 4.0, n),
        "units": "au",
        "unitSI": 1.0,
    }


class TestWriteDeviceData:
    def test_device_data_group_created(self, schema, h5file):
        data = _minimal_3d_data()
        data["device_data"] = {"ecg": _ecg_channel()}
        schema.write(h5file, data)
        assert "device_data" in h5file
        assert isinstance(h5file["device_data"], h5py.Group)
        assert h5file["device_data"].attrs["description"] == (
            "Device signals recorded during this acquisition"
        )

    def test_ecg_channel_attrs(self, schema, h5file):
        data = _minimal_3d_data()
        data["device_data"] = {"ecg": _ecg_channel()}
        schema.write(h5file, data)
        ch = h5file["device_data/ecg"]
        assert ch.attrs["_type"] == "ecg"
        assert int(ch.attrs["_version"]) == 1
        assert ch.attrs["model"] == "GE CardioLab"
        assert ch.attrs["measurement"] == "voltage"
        assert bool(ch.attrs["run_control"]) is True

    def test_ecg_signal_dataset(self, schema, h5file):
        data = _minimal_3d_data()
        ecg = _ecg_channel()
        data["device_data"] = {"ecg": ecg}
        schema.write(h5file, data)
        ds = h5file["device_data/ecg/signal"]
        np.testing.assert_array_almost_equal(ds[:], ecg["signal"])
        assert ds.attrs["units"] == "mV"
        assert ds.attrs["unitSI"] == pytest.approx(0.001)

    def test_ecg_time_dataset(self, schema, h5file):
        data = _minimal_3d_data()
        ecg = _ecg_channel()
        data["device_data"] = {"ecg": ecg}
        schema.write(h5file, data)
        ds = h5file["device_data/ecg/time"]
        np.testing.assert_array_almost_equal(ds[:], ecg["time"])
        assert ds.attrs["units"] == "s"

    def test_ecg_sampling_rate(self, schema, h5file):
        data = _minimal_3d_data()
        data["device_data"] = {"ecg": _ecg_channel()}
        schema.write(h5file, data)
        sr = h5file["device_data/ecg/sampling_rate"]
        assert sr.attrs["value"] == pytest.approx(500.0)
        assert sr.attrs["units"] == "Hz"

    def test_multiple_channels(self, schema, h5file):
        data = _minimal_3d_data()
        data["device_data"] = {
            "ecg": _ecg_channel(),
            "bellows": _bellows_channel(),
        }
        schema.write(h5file, data)
        assert "device_data/ecg" in h5file
        assert "device_data/bellows" in h5file

    def test_no_device_data_when_absent(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "device_data" not in h5file


# ---------------------------------------------------------------------------
# write() — provenance (optional DICOM header and per-slice metadata)
# ---------------------------------------------------------------------------


def _per_slice_metadata_dtype():
    return np.dtype(
        [
            ("instance_number", np.int32),
            ("slice_location", np.float64),
            ("acquisition_time", "S26"),
            ("image_position_patient", np.float64, (3,)),
        ]
    )


class TestWriteProvenance:
    def test_dicom_header_written(self, schema, h5file):
        data = _minimal_3d_data()
        data["provenance"] = {
            "dicom_header": '{"PatientID": "ANON"}',
        }
        schema.write(h5file, data)
        assert "provenance/dicom_header" in h5file
        stored = h5file["provenance/dicom_header"][()]
        if isinstance(stored, bytes):
            stored = stored.decode()
        assert "PatientID" in stored

    def test_dicom_header_description_attr(self, schema, h5file):
        data = _minimal_3d_data()
        data["provenance"] = {"dicom_header": "{}"}
        schema.write(h5file, data)
        ds = h5file["provenance/dicom_header"]
        assert "description" in ds.attrs

    def test_dicom_header_accepts_dict(self, schema, h5file):
        data = _minimal_3d_data()
        data["provenance"] = {"dicom_header": {"PatientID": "ANON"}}
        schema.write(h5file, data)
        stored = h5file["provenance/dicom_header"][()]
        if isinstance(stored, bytes):
            stored = stored.decode()
        import json

        parsed = json.loads(stored)
        assert parsed["PatientID"] == "ANON"

    def test_per_slice_metadata_written(self, schema, h5file):
        data = _minimal_3d_data()
        dt = _per_slice_metadata_dtype()
        slices = np.array(
            [
                (1, -50.0, b"2024-07-24T10:00:00.000000", [0.0, 0.0, -50.0]),
                (2, -49.0, b"2024-07-24T10:00:00.100000", [0.0, 0.0, -49.0]),
            ],
            dtype=dt,
        )
        data["provenance"] = {"per_slice_metadata": slices}
        schema.write(h5file, data)
        assert "provenance/per_slice_metadata" in h5file
        ds = h5file["provenance/per_slice_metadata"]
        assert ds.shape == (2,)
        assert ds.attrs["description"].startswith("Per-slice DICOM metadata")

    def test_no_provenance_when_absent(self, schema, h5file):
        data = _minimal_3d_data()
        schema.write(h5file, data)
        assert "provenance" not in h5file

    def test_both_provenance_fields(self, schema, h5file):
        data = _minimal_3d_data()
        dt = _per_slice_metadata_dtype()
        slices = np.array(
            [
                (1, -50.0, b"2024-07-24T10:00:00.000000", [0.0, 0.0, -50.0]),
            ],
            dtype=dt,
        )
        data["provenance"] = {
            "dicom_header": '{"PatientID": "ANON"}',
            "per_slice_metadata": slices,
        }
        schema.write(h5file, data)
        assert "provenance/dicom_header" in h5file
        assert "provenance/per_slice_metadata" in h5file


# ---------------------------------------------------------------------------
# json_schema() — optional properties
# ---------------------------------------------------------------------------


class TestJsonSchemaOptionalProperties:
    def test_has_mips_property(self, schema):
        result = schema.json_schema()
        assert "mips" in result["properties"]

    def test_has_frames_property(self, schema):
        result = schema.json_schema()
        assert "frames" in result["properties"]

    def test_has_device_data_property(self, schema):
        result = schema.json_schema()
        assert "device_data" in result["properties"]

    def test_has_provenance_property(self, schema):
        result = schema.json_schema()
        assert "provenance" in result["properties"]

    def test_optional_properties_not_required(self, schema):
        result = schema.json_schema()
        required = result.get("required", [])
        for prop in ("mips", "frames", "device_data", "provenance"):
            assert prop not in required


# ---------------------------------------------------------------------------
# Entry point registration
# ---------------------------------------------------------------------------


class TestEntryPointRegistration:
    def test_factory_returns_recon_schema(self):
        from fd5.imaging.recon import ReconSchema

        instance = ReconSchema()
        assert instance.product_type == "recon"

    def test_entry_point_name_is_recon(self):
        """The entry point must register under name 'recon'."""
        import importlib.metadata

        eps = importlib.metadata.entry_points(group="fd5.schemas")
        names = [ep.name for ep in eps]
        assert "recon" in names


# ---------------------------------------------------------------------------
# Integration test
# ---------------------------------------------------------------------------


class TestIntegration:
    def test_create_validate_roundtrip(self, schema, h5path):
        from fd5.schema import embed_schema, validate

        data = _minimal_3d_data()
        with h5py.File(h5path, "w") as f:
            root_attrs = schema.required_root_attrs()
            for k, v in root_attrs.items():
                f.attrs[k] = v
            f.attrs["name"] = "integration-test-recon"
            f.attrs["description"] = "Integration test recon file"
            schema_dict = schema.json_schema()
            embed_schema(f, schema_dict)
            schema.write(f, data)

        errors = validate(h5path)
        assert errors == [], [e.message for e in errors]

    def test_generate_schema_for_recon(self, schema):
        register_schema("recon", schema)
        from fd5.schema import generate_schema

        result = generate_schema("recon")
        assert result["$schema"] == "https://json-schema.org/draft/2020-12/schema"
        assert result["properties"]["product"]["const"] == "recon"
