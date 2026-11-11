// tessera-capi C++ smoke test: write a listmode-like table through the C ABI, from nanoarrow
// batches, and dump the columns it pushed so a Rust reader can compare them value for value.
//
//   capi_smoke <stage_dir> <out.tsra> <dump_dir> [rows] [batch_rows] [--unknown-rows]
//
// Checks on the way: the ABI version matches the header, a mismatched batch is refused with
// TSRA_STATUS_SCHEMA and is still released, a duplicate sequence number is TSRA_STATUS_ORDER, and
// the library calls each pushed array's release callback exactly once. Prints `content_hash <h>`.
#include "listmode.hpp"

#include <cstring>
#include <fstream>

using namespace listmode;

namespace {

template <typename T>
void dump(const std::string& path, uint64_t rows, T (*f)(uint64_t)) {
  std::ofstream out(path, std::ios::binary);
  for (uint64_t i = 0; i < rows; ++i) {
    const T v = f(i);
    out.write(reinterpret_cast<const char*>(&v), sizeof v);
  }
  if (!out) die("write " + path);
}

void dump_validity(const std::string& path, uint64_t rows) {
  std::ofstream out(path, std::ios::binary);
  for (uint64_t i = 0; i < rows; ++i) out.put(tof_valid(i) ? 1 : 0);
  if (!out) die("write " + path);
}

// A one-column batch that doesn't fit the declared schema.
void push_mismatched(TsraTable* table, int64_t& pushed) {
  ArrowSchema schema;
  ArrowSchemaInit(&schema);
  check_na(ArrowSchemaSetTypeStruct(&schema, 1), "struct");
  check_na(ArrowSchemaSetType(schema.children[0], NANOARROW_TYPE_INT16), "type");
  check_na(ArrowSchemaSetName(schema.children[0], "t"), "name");
  ArrowArray array;
  ArrowError error;
  check_na(ArrowArrayInitFromSchema(&array, &schema, &error), "init");
  check_na(ArrowArrayStartAppending(&array), "start");
  check_na(ArrowArrayAppendInt(array.children[0], 1), "append");
  check_na(ArrowArrayFinishElement(&array), "element");
  check_na(ArrowArrayFinishBuildingDefault(&array, &error), "finish");
  count_release(&array);
  ++pushed;
  const TsraStatus st = tsra_table_push(table, 0, &array, &schema);
  if (st != TSRA_STATUS_SCHEMA) die("a mismatched batch must be TSRA_STATUS_SCHEMA");
  if (array.release != nullptr) die("the library must take ownership of a refused batch");
  std::printf("refused as expected: %s\n", tsra_last_error());
  ArrowSchemaRelease(&schema);
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 4) die("usage: capi_smoke <stage_dir> <out.tsra> <dump_dir> [rows] [batch_rows] [--unknown-rows]");
  const std::string stage = argv[1], out = argv[2], dumps = argv[3];
  const uint64_t rows = argc > 4 ? std::strtoull(argv[4], nullptr, 10) : 200000;
  const int64_t batch_rows = argc > 5 ? std::strtoll(argv[5], nullptr, 10) : 9973;
  const bool unknown = argc > 6 && std::strcmp(argv[6], "--unknown-rows") == 0;

  const uint32_t abi = tsra_abi_version();
  if ((abi >> 16) != TSRA_ABI_VERSION_MAJOR) die("ABI major mismatch between header and library");

  ArrowSchema schema;
  make_schema(&schema);

  const TsraSessionOptions sopts = session_options(2);
  TsraSession* session = nullptr;
  check(tsra_session_open(stage.c_str(), &sopts, &session), "tsra_session_open");
  const TsraTableOptions topts = table_options(unknown ? TSRA_ROWS_UNKNOWN : rows);
  TsraTable* table = nullptr;
  check(tsra_table_begin(session, "events", &schema, &topts, &table), "tsra_table_begin");

  int64_t pushed = 0;
  push_mismatched(table, pushed);

  uint64_t seq = 0;
  for (uint64_t first = 0; first < rows; first += static_cast<uint64_t>(batch_rows), ++seq) {
    const int64_t n = static_cast<int64_t>(std::min<uint64_t>(batch_rows, rows - first));
    ArrowArray array;
    make_batch(&schema, first, n, &array);
    count_release(&array);
    ++pushed;
    check(tsra_table_push(table, seq, &array, &schema), "tsra_table_push");
    if (array.release != nullptr) die("the library must take ownership of a pushed batch");
    if (seq == 0) {
      // A duplicate sequence number is refused, and the batch is still released.
      ArrowArray dup;
      make_batch(&schema, 0, n, &dup);
      count_release(&dup);
      ++pushed;
      if (tsra_table_push(table, 0, &dup, &schema) != TSRA_STATUS_ORDER) die("a duplicate seq must be TSRA_STATUS_ORDER");
    }
  }
  if (released_count() != pushed) die("release calls " + std::to_string(released_count()) + " != pushes " + std::to_string(pushed));
  check(tsra_table_end(table), "tsra_table_end");

  char hash[TSRA_HASH_CAPACITY];
  check(tsra_session_seal(session, out.c_str(), hash, sizeof hash), "tsra_session_seal");
  ArrowSchemaRelease(&schema);

  dump<uint64_t>(dumps + "/t.bin", rows, t_of);
  dump<uint32_t>(dumps + "/crystal_a.bin", rows, crystal_a_of);
  dump<uint32_t>(dumps + "/crystal_b.bin", rows, crystal_b_of);
  dump<float>(dumps + "/energy.bin", rows, energy_of);
  dump<float>(dumps + "/tof.bin", rows, tof_of);
  dump_validity(dumps + "/tof.valid", rows);

  std::printf("pushed %lld batches (%lld release calls), %llu rows\n", static_cast<long long>(pushed),
              static_cast<long long>(released_count()), static_cast<unsigned long long>(rows));
  std::printf("content_hash %s\n", hash);
  return 0;
}
