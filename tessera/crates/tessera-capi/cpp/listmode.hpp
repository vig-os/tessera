// A listmode-like event table built with nanoarrow, shared by the tessera-capi C++ programs.
//
// Every value is a pure function of the event's global row index, so a batch is the same whoever
// builds it and in whichever thread: which is what lets the threaded program compare content hashes.
//
// Columns: t (u64, ps), crystal_a / crystal_b (u32), energy (f32, keV) and tof (f32, nullable: NULL
// for every 13th event, a coincidence without a time-of-flight measurement).
#pragma once

#include <nanoarrow/nanoarrow.h>
#include <tessera.h>

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <stdexcept>
#include <string>
#include <vector>

namespace listmode {

constexpr int kColumns = 5;

inline uint64_t t_of(uint64_t i) { return i * 7919u + 3u; }
inline uint32_t crystal_a_of(uint64_t i) { return static_cast<uint32_t>((i * 2654435761u) % 20160u); }
inline uint32_t crystal_b_of(uint64_t i) { return static_cast<uint32_t>((i * 40503u + 17u) % 20160u); }
inline float energy_of(uint64_t i) { return static_cast<float>((i * 7u) % 6500u) * 0.1f + 0.05f; }
inline bool tof_valid(uint64_t i) { return i % 13u != 0u; }
inline float tof_of(uint64_t i) { return static_cast<float>(static_cast<int64_t>(i % 801u) - 400) * 1.5f; }

[[noreturn]] inline void die(const std::string& what) {
  std::fprintf(stderr, "FAIL: %s\n", what.c_str());
  std::exit(1);
}

inline void check_na(ArrowErrorCode code, const char* what) {
  if (code != NANOARROW_OK) die(std::string("nanoarrow: ") + what);
}

// A tsra call that must succeed; on failure, print the library's last error and exit.
inline void check(TsraStatus status, const char* what) {
  if (status != TSRA_STATUS_OK) {
    const char* msg = tsra_last_error();
    die(std::string(what) + " -> status " + std::to_string(static_cast<int>(status)) + ": " +
        (msg ? msg : "(no message)"));
  }
}

// The table's schema: a struct of the five columns. The caller owns it (ArrowSchemaRelease).
inline void make_schema(ArrowSchema* schema) {
  ArrowSchemaInit(schema);
  check_na(ArrowSchemaSetTypeStruct(schema, kColumns), "struct schema");
  const struct {
    const char* name;
    ArrowType type;
    bool nullable;
  } cols[kColumns] = {
      {"t", NANOARROW_TYPE_UINT64, false},       {"crystal_a", NANOARROW_TYPE_UINT32, false},
      {"crystal_b", NANOARROW_TYPE_UINT32, false}, {"energy", NANOARROW_TYPE_FLOAT, false},
      {"tof", NANOARROW_TYPE_FLOAT, true},
  };
  for (int c = 0; c < kColumns; ++c) {
    check_na(ArrowSchemaSetType(schema->children[c], cols[c].type), "column type");
    check_na(ArrowSchemaSetName(schema->children[c], cols[c].name), "column name");
    schema->children[c]->flags = cols[c].nullable ? ARROW_FLAG_NULLABLE : 0;
  }
  // The top-level struct is a batch of rows, never NULL itself.
  schema->flags = 0;
}

template <typename T>
inline void append_values(ArrowArray* child, uint64_t first, int64_t n, T (*f)(uint64_t)) {
  ArrowBuffer* data = ArrowArrayBuffer(child, 1);
  check_na(ArrowBufferReserve(data, n * static_cast<int64_t>(sizeof(T))), "reserve");
  for (int64_t k = 0; k < n; ++k) {
    const T v = f(first + static_cast<uint64_t>(k));
    ArrowBufferAppendUnsafe(data, &v, sizeof(T));
  }
  child->length = n;
}

// Rows [first, first + n) as one struct ArrowArray, built buffer-wise (not element-wise). The
// caller owns it: push it to tessera (which takes ownership) or ArrowArrayRelease it.
inline void make_batch(const ArrowSchema* schema, uint64_t first, int64_t n, ArrowArray* out) {
  ArrowError error;
  check_na(ArrowArrayInitFromSchema(out, schema, &error), "init array");
  append_values<uint64_t>(out->children[0], first, n, t_of);
  append_values<uint32_t>(out->children[1], first, n, crystal_a_of);
  append_values<uint32_t>(out->children[2], first, n, crystal_b_of);
  append_values<float>(out->children[3], first, n, energy_of);
  ArrowArray* tof = out->children[4];
  append_values<float>(tof, first, n, tof_of);
  ArrowBitmap* validity = ArrowArrayValidityBitmap(tof);
  check_na(ArrowBitmapReserve(validity, n), "reserve validity");
  int64_t nulls = 0;
  for (int64_t k = 0; k < n; ++k) {
    const bool valid = tof_valid(first + static_cast<uint64_t>(k));
    ArrowBitmapAppendUnsafe(validity, valid ? 1 : 0, 1);
    nulls += valid ? 0 : 1;
  }
  tof->null_count = nulls;
  out->length = n;
  out->null_count = 0;
  check_na(ArrowArrayFinishBuildingDefault(out, &error), "finish array");
}

// Count release callbacks: wrap an array's release so the test can prove the library calls it
// exactly once per pushed batch, on success and on failure.
inline std::atomic<int64_t> g_released{0};

struct CountedRelease {
  void (*inner)(ArrowArray*);
  void* inner_private;
};

inline void counted_release(ArrowArray* array) {
  auto* wrap = static_cast<CountedRelease*>(array->private_data);
  array->private_data = wrap->inner_private;
  array->release = wrap->inner;
  array->release(array);
  delete wrap;
  g_released.fetch_add(1);
}

inline int64_t released_count() { return g_released.load(); }

inline void count_release(ArrowArray* array) {
  array->private_data = new CountedRelease{array->release, array->private_data};
  array->release = counted_release;
}

inline TsraSessionOptions session_options(uint32_t workers) {
  TsraSessionOptions o{};
  o.struct_size = sizeof(TsraSessionOptions);
  o.product = "listmode";
  o.name = "capi-smoke";
  o.description = "listmode-like coincidences written from C++ through tessera-capi";
  o.timestamp = "2026-11-11T00:00:00Z";
  o.workers = workers;
  return o;
}

inline TsraTableOptions table_options(uint64_t rows) {
  TsraTableOptions o{};
  o.struct_size = sizeof(TsraTableOptions);
  o.rows = rows;
  o.reorder_window = 0;
  return o;
}

}  // namespace listmode
