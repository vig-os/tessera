"""Tests for pre-seal validation in fd5.create."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import h5py
import numpy as np
import pytest

from fd5.create import Fd5ValidationError, create
from fd5.registry import register_schema


# ---------------------------------------------------------------------------
# Stub schemas
# ---------------------------------------------------------------------------


class _ValidatingStubSchema:
    """Schema that requires a 'values' dataset in its JSON schema."""

    product_type: str = "test/validating"
    schema_version: str = "1.0.0"

    def json_schema(self) -> dict[str, Any]:
        return {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "_schema_version": {"type": "integer"},
                "product": {"type": "string", "const": "test/validating"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "timestamp": {"type": "string"},
                "values": {"type": "object"},
            },
            "required": ["_schema_version", "product", "name", "values"],
        }

    def required_root_attrs(self) -> dict[str, Any]:
        return {"product": "test/validating"}

    def write(self, target: Any, data: Any) -> None:
        target.create_dataset("volume", data=data)

    def id_inputs(self) -> list[str]:
        return ["product", "name", "timestamp"]


class _SimpleStubSchema:
    """Minimal schema without custom validate -- for baseline tests."""

    product_type: str = "test/simple"
    schema_version: str = "1.0.0"

    def json_schema(self) -> dict[str, Any]:
        return {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "_schema_version": {"type": "integer"},
                "product": {"type": "string", "const": "test/simple"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "timestamp": {"type": "string"},
            },
            "required": ["_schema_version", "product", "name"],
        }

    def required_root_attrs(self) -> dict[str, Any]:
        return {"product": "test/simple"}

    def write(self, target: Any, data: Any) -> None:
        if data is not None:
            target.create_dataset("volume", data=data)

    def id_inputs(self) -> list[str]:
        return ["product", "name", "timestamp"]


class _CustomValidateSchema(_SimpleStubSchema):
    """Schema with a custom validate() method."""

    product_type: str = "test/custom-validate"
    call_count: int = 0

    def json_schema(self) -> dict[str, Any]:
        schema = super().json_schema()
        schema["properties"]["product"]["const"] = "test/custom-validate"
        return schema

    def required_root_attrs(self) -> dict[str, Any]:
        return {"product": "test/custom-validate"}

    def validate(self, target: Any) -> None:
        self.call_count += 1


class _RejectingValidateSchema(_SimpleStubSchema):
    """Schema whose validate() always raises."""

    product_type: str = "test/rejecting"

    def json_schema(self) -> dict[str, Any]:
        schema = super().json_schema()
        schema["properties"]["product"]["const"] = "test/rejecting"
        return schema

    def required_root_attrs(self) -> dict[str, Any]:
        return {"product": "test/rejecting"}

    def validate(self, target: Any) -> None:
        raise Fd5ValidationError("Product-specific validation failed")


@pytest.fixture(autouse=True)
def _register_stubs():
    import fd5.registry as reg

    register_schema("test/validating", _ValidatingStubSchema())
    register_schema("test/simple", _SimpleStubSchema())
    _custom = _CustomValidateSchema()
    register_schema("test/custom-validate", _custom)
    register_schema("test/rejecting", _RejectingValidateSchema())
    reg._ep_loaded = True
    yield _custom


@pytest.fixture()
def out_dir(tmp_path: Path) -> Path:
    return tmp_path


# ---------------------------------------------------------------------------
# Schema validation
# ---------------------------------------------------------------------------


class TestSchemaValidation:
    def test_schema_validation_catches_missing_dataset(self, out_dir: Path):
        """Schema requires 'values' group but we don't write it."""
        with pytest.raises(Fd5ValidationError, match="Schema validation failed"):
            with create(
                out_dir,
                product="test/validating",
                name="sample",
                description="A test file for validation",
                timestamp="2026-02-25T12:00:00Z",
            ):
                pass  # don't write the required 'values' group

    def test_schema_validation_passes_valid_file(self, out_dir: Path):
        """Normal create + seal works without errors."""
        with create(
            out_dir,
            product="test/simple",
            name="sample",
            description="A valid test file with enough description",
            timestamp="2026-02-25T12:00:00Z",
        ) as builder:
            builder.write_product(np.zeros((4, 4), dtype=np.float32))

        finals = list(out_dir.glob("*.h5"))
        assert len(finals) == 1


# ---------------------------------------------------------------------------
# Description quality warnings
# ---------------------------------------------------------------------------


class TestDescriptionQuality:
    def test_description_quality_warns_short(self, out_dir: Path):
        """File with a group whose description is short triggers a warning."""
        with pytest.warns(UserWarning, match="Short description"):
            with create(
                out_dir,
                product="test/simple",
                name="sample",
                description="A sufficiently long description for the root",
                timestamp="2026-02-25T12:00:00Z",
            ) as builder:
                grp = builder.file.create_group("mygroup")
                grp.attrs["description"] = "tiny"

    def test_description_quality_does_not_block(self, out_dir: Path):
        """File with quality warnings still seals successfully."""
        with pytest.warns(UserWarning, match="Short description"):
            with create(
                out_dir,
                product="test/simple",
                name="sample",
                description="A sufficiently long description for the root",
                timestamp="2026-02-25T12:00:00Z",
            ) as builder:
                grp = builder.file.create_group("mygroup")
                grp.attrs["description"] = "tiny"

        finals = list(out_dir.glob("*.h5"))
        assert len(finals) == 1


# ---------------------------------------------------------------------------
# Product-specific validation
# ---------------------------------------------------------------------------


class TestProductValidate:
    def test_product_validate_method_called(self, out_dir: Path, _register_stubs):
        """Schema with custom validate() method is called during seal."""
        schema = _register_stubs
        schema.call_count = 0
        with create(
            out_dir,
            product="test/custom-validate",
            name="sample",
            description="A test file with custom validation",
            timestamp="2026-02-25T12:00:00Z",
        ):
            pass
        assert schema.call_count == 1

    def test_product_validate_can_reject(self, out_dir: Path):
        """Schema validate() raises -> seal aborts."""
        with pytest.raises(Fd5ValidationError, match="Product-specific validation"):
            with create(
                out_dir,
                product="test/rejecting",
                name="sample",
                description="A test file that will be rejected",
                timestamp="2026-02-25T12:00:00Z",
            ):
                pass


# ---------------------------------------------------------------------------
# Pre-seal hooks
# ---------------------------------------------------------------------------


class TestPreSealHooks:
    def test_pre_seal_hooks_called(self, out_dir: Path):
        """Hook is called with the open h5py.File."""
        called_with = []

        def hook(f: h5py.File) -> None:
            called_with.append(f.filename)

        with create(
            out_dir,
            product="test/simple",
            name="sample",
            description="A test file with hooks enabled",
            timestamp="2026-02-25T12:00:00Z",
            pre_seal_hooks=[hook],
        ):
            pass

        assert len(called_with) == 1

    def test_pre_seal_hook_can_abort(self, out_dir: Path):
        """Hook raises exception -> seal aborts, temp file cleaned up."""

        def bad_hook(f: h5py.File) -> None:
            raise RuntimeError("hook abort")

        with pytest.raises(RuntimeError, match="hook abort"):
            with create(
                out_dir,
                product="test/simple",
                name="sample",
                description="A test file that hook will abort",
                timestamp="2026-02-25T12:00:00Z",
                pre_seal_hooks=[bad_hook],
            ):
                pass

        # Temp file should be cleaned up
        h5_files = list(out_dir.glob("*.h5"))
        tmp_files = list(out_dir.glob("*.h5.tmp"))
        assert len(h5_files) == 0
        assert len(tmp_files) == 0

    def test_no_hooks_default(self, out_dir: Path):
        """Default behavior unchanged -- no hooks, no warnings for good files."""
        with create(
            out_dir,
            product="test/simple",
            name="sample",
            description="A sufficiently long description for quality checks",
            timestamp="2026-02-25T12:00:00Z",
        ) as builder:
            builder.write_product(np.zeros((4, 4), dtype=np.float32))

        finals = list(out_dir.glob("*.h5"))
        assert len(finals) == 1
